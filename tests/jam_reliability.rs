use bstr::BString;
use jamjam::jam::last_read_storage::JamLastReadStorage;
use jamjam::jam::pack::PackOptions;
use jamjam::jam::{JamMessage, JamMessageBase, raw};
use jamjam::util::echomail::EchomailAddress;
use std::collections::BTreeMap;
use tempfile::TempDir;

fn message(text: &str) -> JamMessage {
    JamMessage::new(&EchomailAddress::default())
        .with_to(BString::from("all"))
        .with_subject(BString::from(text))
        .with_text(BString::from(text))
}

fn next_random(state: &mut u64) -> u32 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (*state >> 32) as u32
}

fn verify(base: &JamMessageBase, high: u32, messages: &BTreeMap<u32, (String, bool)>) {
    let active = messages.values().filter(|(_, deleted)| !deleted).count() as u32;
    assert_eq!(base.active_messages(), active);
    assert_eq!(base.highest_message_number(), high);

    for number in 1..=high {
        match messages.get(&number) {
            Some((text, false)) => {
                let header = base.read_header(number).unwrap();
                assert_eq!(header.message_number, number);
                assert_eq!(base.read_message_text(&header).unwrap(), text.as_str());
            }
            _ => assert!(base.read_header(number).is_err()),
        }
    }
}

/// Exercises a reproducible mix of writes, deletes, restores, header updates,
/// packing and reopening against a simple in-memory model.
#[test]
fn test_long_mixed_operation_sequence_matches_model() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("sequence");
    let mut base = JamMessageBase::create(&path).unwrap();
    let mut messages: BTreeMap<u32, (String, bool)> = BTreeMap::new();
    let mut high = 0;
    let mut random = 0xC0FF_EE12_3456_7890;

    for step in 0..250 {
        let choice = next_random(&mut random) % 10;
        match choice {
            0..=3 => {
                high += 1;
                let text = format!("message-{step}-{high}");
                assert_eq!(base.write_message(&message(&text)).unwrap(), high);
                messages.insert(high, (text, false));
            }
            4 if high > 0 => {
                let number = next_random(&mut random) % high + 1;
                if let Some((_, deleted)) = messages.get_mut(&number)
                    && !*deleted
                {
                    base.delete_message(number).unwrap();
                    *deleted = true;
                }
            }
            5 if high > 0 => {
                let number = next_random(&mut random) % high + 1;
                if let Some((_, deleted)) = messages.get_mut(&number)
                    && *deleted
                {
                    base.restore_message(number).unwrap();
                    *deleted = false;
                }
            }
            6 if high > 0 => {
                let number = next_random(&mut random) % high + 1;
                if let Some((_, false)) = messages.get(&number) {
                    let mut header = base.read_header(number).unwrap();
                    header.set_subject(BString::from(format!("updated-{step}")));
                    raw::update_header(&mut base, number, &header).unwrap();
                }
            }
            7 => {
                base.pack(&PackOptions::default()).unwrap();
                messages.retain(|_, (_, deleted)| !*deleted);
            }
            8 => {
                drop(base);
                base = JamMessageBase::open(&path).unwrap();
            }
            _ => base.sync().unwrap(),
        }
        verify(&base, high, &messages);
    }

    drop(base);
    let base = JamMessageBase::open(path).unwrap();
    verify(&base, high, &messages);
}

/// Same mixed sequence as the model test, plus last-read writes and a verify
/// after every operation. The seed is fixed so a failure is reproducible.
#[test]
fn test_seeded_sequence_with_last_read_and_verify() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("seeded");
    let mut base = JamMessageBase::create(&path).unwrap();
    let mut messages: BTreeMap<u32, (String, bool)> = BTreeMap::new();
    let mut last_read = 0u32;
    let mut high = 0;
    let mut random = 0xA11C_E0DE_5EED_0001;

    for step in 0..200 {
        let choice = next_random(&mut random) % 12;
        match choice {
            0..=3 => {
                high += 1;
                let text = format!("seeded-{step}-{high}");
                assert_eq!(base.write_message(&message(&text)).unwrap(), high);
                messages.insert(high, (text, false));
            }
            4 if high > 0 => {
                let number = next_random(&mut random) % high + 1;
                if let Some((_, deleted)) = messages.get_mut(&number)
                    && !*deleted
                {
                    base.delete_message(number).unwrap();
                    *deleted = true;
                }
            }
            5 if high > 0 => {
                let number = next_random(&mut random) % high + 1;
                if let Some((_, deleted)) = messages.get_mut(&number)
                    && *deleted
                {
                    base.restore_message(number).unwrap();
                    *deleted = false;
                }
            }
            6 if high > 0 => {
                last_read = next_random(&mut random) % high + 1;
                base.write_last_read(&JamLastReadStorage {
                    user_crc: 7,
                    user_id: 8,
                    last_read_msg: last_read,
                    high_read_msg: last_read,
                })
                .unwrap();
            }
            7 => {
                base.pack(&PackOptions::default()).unwrap();
                messages.retain(|_, (_, deleted)| !*deleted);
            }
            8 => {
                drop(base);
                base = JamMessageBase::open(&path).unwrap();
            }
            _ => base.sync().unwrap(),
        }
        verify(&base, high, &messages);
        let report = base.verify().unwrap();
        assert!(
            report.is_ok()
                || report.problems.iter().all(|problem| matches!(
                    problem,
                    jamjam::jam::verify::Problem::UnreferencedText { .. }
                )),
            "step {step}: {report}"
        );
        if last_read > 0 {
            let stored = base.find_last_read(7, 8).unwrap().unwrap();
            assert!(stored.last_read_msg <= base.highest_message_number());
        }
    }
}

#[test]
fn test_pack_renumber_round_trip_keeps_live_message_order() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("renumber");
    let mut base = JamMessageBase::create(&path).unwrap();
    for number in 1..=40 {
        base.write_message(&message(&format!("message-{number}")))
            .unwrap();
        if number % 3 == 0 {
            base.delete_message(number).unwrap();
        }
    }

    let expected: Vec<_> = (1..=40)
        .filter(|number| number % 3 != 0)
        .map(|number| format!("message-{number}"))
        .collect();
    base.pack(&PackOptions {
        renumber_from: Some(500),
        ..Default::default()
    })
    .unwrap();
    drop(base);

    let base = JamMessageBase::open(path).unwrap();
    assert_eq!(base.active_messages(), expected.len() as u32);
    assert_eq!(base.lowest_message_number(), 500);
    assert_eq!(
        base.highest_message_number(),
        500 + expected.len() as u32 - 1
    );
    for (index, text) in expected.iter().enumerate() {
        let header = base.read_header(500 + index as u32).unwrap();
        assert_eq!(base.read_message_text(&header).unwrap(), text.as_str());
    }
}
