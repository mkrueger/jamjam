//! Mutation fuzzing of the parsers with a fixed seed.
//!
//! Valid data is mutated by a deterministic PRNG and fed back in. A parser may
//! reject the result, but it must not panic, hang or accept something it cannot
//! encode again, so every case that survives a parse is round-tripped. Deeper
//! coverage-guided runs live in `fuzz/`, this suite is the regression net that
//! runs on every build.

use bstr::{BString, ByteSlice};
use jamjam::jam::jhr_header::JhrHeaderInfo;
use jamjam::jam::last_read_storage::JamLastReadStorage;
use jamjam::jam::msg_header::{JamMessageHeader, MessageSubfield, SubfieldType};
use jamjam::jam::{JamMessage, JamMessageBase};
use jamjam::pcboard::{PCBoardMessage, PCBoardMessageBase};
use jamjam::qwk::QwkMessageBase;
use jamjam::qwk::control::ControlDat;
use jamjam::qwk::qwk_message::{MSG_ACTIVE, QwkMessage};
use jamjam::util::echomail::EchomailAddress;
use std::path::Path;
use tempfile::TempDir;

/// xorshift64, so a failure can be replayed from the seed printed by the test.
struct Fuzzer(u64);

impl Fuzzer {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            self.next_u32() as usize % bound
        }
    }

    fn pick<'a, T>(&mut self, values: &'a [T]) -> &'a T {
        &values[self.below(values.len())]
    }

    /// Applies a handful of edits that target what the formats trust: lengths,
    /// offsets, record alignment and the end of the file.
    fn mutate(&mut self, seed: &[u8]) -> Vec<u8> {
        let mut data = seed.to_vec();
        for _ in 0..=self.below(4) {
            match self.next_u32() % 7 {
                0 if !data.is_empty() => {
                    let index = self.below(data.len());
                    data[index] ^= 1 << self.below(8);
                }
                1 if !data.is_empty() => {
                    let index = self.below(data.len());
                    data[index] = self.next_u32() as u8;
                }
                2 if data.len() >= 4 => {
                    const WILD: [u32; 6] = [0, 1, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFE, u32::MAX];
                    let index = self.below(data.len() - 3);
                    let value = *self.pick(&WILD);
                    data[index..index + 4].copy_from_slice(&value.to_le_bytes());
                }
                3 if !data.is_empty() => {
                    let len = self.below(data.len());
                    data.truncate(len);
                }
                4 => {
                    for _ in 0..self.below(64) {
                        data.push(self.next_u32() as u8);
                    }
                }
                5 if !data.is_empty() => {
                    let from = self.below(data.len());
                    let len = self.below(data.len() - from);
                    let chunk = data[from..from + len].to_vec();
                    let at = self.below(data.len());
                    data.splice(at..at, chunk);
                }
                _ if !data.is_empty() => {
                    let from = self.below(data.len());
                    let len = self.below(data.len() - from);
                    data[from..from + len].fill(0);
                }
                _ => {}
            }
        }
        data
    }
}

fn sample_base(path: &Path) -> JamMessageBase {
    let mut base = JamMessageBase::create(path).unwrap();
    for index in 0..6u32 {
        let message = JamMessage::new(&EchomailAddress::default())
            .with_from(BString::from("sysop"))
            .with_to(BString::from("all"))
            .with_subject(BString::from(format!("subject {index}")))
            .with_text(BString::from("text ".repeat(index as usize + 1)))
            .with_msg_id(BString::from(format!("1:2/3 {index:08x}")))
            .with_reply_id(BString::from("1:2/3 00000000"))
            .with_sub_field(MessageSubfield::new(
                SubfieldType::Path2D,
                BString::from("2/3 4/5"),
            ));
        base.write_message(&message).unwrap();
    }
    base.delete_message(3).unwrap();
    base.write_last_read(&JamLastReadStorage {
        user_crc: JamMessageBase::crc(&BString::from("sysop")),
        user_id: 1,
        last_read_msg: 2,
        high_read_msg: 5,
    })
    .unwrap();
    base
}

const JAM_EXTENSIONS: [&str; 4] = ["jhr", "jdt", "jdx", "jlr"];

/// The four base files of a healthy JAM base, ready to be mutated.
fn jam_base_seed() -> Vec<(&'static str, Vec<u8>)> {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("seed");
    drop(sample_base(&path));
    JAM_EXTENSIONS
        .iter()
        .map(|extension| {
            let bytes = std::fs::read(path.with_extension(extension)).unwrap_or_default();
            (*extension, bytes)
        })
        .collect()
}

/// Header records of a generated and of a real base, without the file header.
fn jam_header_seeds() -> Vec<Vec<u8>> {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("seed");
    drop(sample_base(&path));

    let skip = JhrHeaderInfo::JHR_HEADER_SIZE as usize;
    [
        std::fs::read(path.with_extension("jhr")).unwrap(),
        std::fs::read("data/jam/general.jhr").unwrap(),
    ]
    .iter()
    .map(|bytes| bytes[skip.min(bytes.len())..].to_vec())
    .collect()
}

#[test]
fn test_fuzz_jam_header_round_trips_what_it_accepts() {
    let mut fuzzer = Fuzzer::new(0x5EED_1A11);
    let mut accepted = 0;

    for seed in jam_header_seeds() {
        for _ in 0..3000 {
            let data = fuzzer.mutate(&seed);
            let Ok(header) = JamMessageHeader::read(&mut data.as_slice()) else {
                continue;
            };
            accepted += 1;

            let mut encoded = Vec::new();
            header
                .write(&mut encoded)
                .expect("an accepted header must encode again");
            let again = JamMessageHeader::read(&mut encoded.as_slice())
                .expect("an encoded header must parse again");

            assert_eq!(again.message_number, header.message_number);
            assert_eq!(again.offset, header.offset);
            assert_eq!(again.txt_len, header.txt_len);
            assert_eq!(again.attributes, header.attributes);
            assert_eq!(again.msgid_crc, header.msgid_crc);
            assert_eq!(again.password_crc, header.password_crc);
            assert_eq!(again.sub_fields.len(), header.sub_fields.len());
            for (left, right) in again.sub_fields.iter().zip(&header.sub_fields) {
                assert_eq!(left.field_type(), right.field_type());
                assert_eq!(left.content(), right.content());
            }
        }
    }

    assert!(
        accepted > 500,
        "only {accepted} mutated headers were accepted"
    );
}

#[test]
fn test_fuzz_jam_base_survives_damaged_files() {
    let seed = jam_base_seed();
    let mut fuzzer = Fuzzer::new(0xDEAD_BEEF_CAFE);
    let mut opened = 0;

    for round in 0..240 {
        let damaged = *fuzzer.pick(&JAM_EXTENSIONS);
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("fuzz");
        for (extension, bytes) in &seed {
            let data = if *extension == damaged {
                fuzzer.mutate(bytes)
            } else {
                bytes.clone()
            };
            std::fs::write(path.with_extension(extension), data).unwrap();
        }

        let Ok(mut base) = JamMessageBase::open(&path) else {
            continue;
        };
        opened += 1;

        let _ = base.verify();
        let low = base.lowest_message_number();
        let high = base.highest_message_number();
        for header in base.messages().flatten() {
            assert!(
                (low..=high).contains(&header.message_number),
                "message {} outside {low}..={high} after damaging .{damaged}",
                header.message_number
            );
        }
        for message in base.messages_full() {
            let _ = message;
        }
        let _ = base.read_last_read_file();
        let _ = base.search_to(&BString::from("all"));
        let _ = base.find_by_msgid(&BString::from("1:2/3 00000000"));
        for number in low..=low.saturating_add(8) {
            let _ = base.read_header(number);
        }

        // Repair rewrites the base, so a base it calls clean has to read back.
        if round % 3 == 0
            && let Ok(report) = base.repair()
        {
            drop(base);
            let mut repaired = JamMessageBase::open(&path).unwrap_or_else(|err| {
                panic!("repaired base does not open after .{damaged}: {err}")
            });
            let after = repaired.verify().unwrap_or_else(|err| {
                panic!("repaired base does not verify after .{damaged}: {err}")
            });
            assert_eq!(
                after, report.after,
                "reopening disagrees with the repair report after .{damaged}"
            );
            if after.is_ok() {
                for message in repaired.messages_full() {
                    message.unwrap_or_else(|err| {
                        panic!("clean base has an unreadable message after .{damaged}: {err}")
                    });
                }
            }
        }
    }

    assert!(opened > 60, "only {opened} damaged bases opened");
}

#[test]
fn test_fuzz_pcboard_reader_rejects_damaged_bases() {
    let seed = std::fs::read("data/pcboard/test").unwrap();
    let mut fuzzer = Fuzzer::new(0x9CB0_A4D5);
    let mut opened = 0;

    for _ in 0..400 {
        let data = fuzzer.mutate(&seed);
        let _ = PCBoardMessage::read(&mut data.as_slice());

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("fuzz");
        std::fs::write(&path, &data).unwrap();
        let Ok(base) = PCBoardMessageBase::open(&path) else {
            continue;
        };
        opened += 1;
        let _ = base.read_message(base.lowest_message_number());
        if let Ok(messages) = base.iter() {
            for message in messages.take(256) {
                let _ = message;
            }
        }
    }

    assert!(opened > 60, "only {opened} damaged bases opened");
}

fn qwk_message_seeds() -> Vec<Vec<u8>> {
    let message = QwkMessage {
        status: b' ',
        msg_number: 12,
        date_time: BString::from("01-01-9112:30"),
        to: BString::from("A VERY LONG RECIPIENT NAME THAT EXCEEDS THE FIELD"),
        from: BString::from("SYSOP"),
        subj: BString::from("test"),
        password: BString::from(""),
        ref_msg_number: 3,
        active_flag: MSG_ACTIVE,
        conference_number: 0,
        logical_message_number: 1,
        net_tag: b' ',
        text: BString::from("hello\nworld\n"),
    };

    [false, true]
        .iter()
        .map(|extended| {
            let mut buffer = Vec::new();
            message.write(&mut buffer, *extended).unwrap();
            buffer
        })
        .collect()
}

#[test]
fn test_fuzz_qwk_message_round_trips_what_it_accepts() {
    let mut fuzzer = Fuzzer::new(0x0FF1_CE05);
    let mut accepted = 0;

    for seed in qwk_message_seeds() {
        for _ in 0..1500 {
            let data = fuzzer.mutate(&seed);
            for extended in [false, true] {
                let Ok(message) = QwkMessage::read(data.as_slice(), extended) else {
                    continue;
                };
                accepted += 1;

                let mut encoded = Vec::new();
                if message.write(&mut encoded, extended).is_err() {
                    // Values that do not fit their fixed slot are rejected on write.
                    continue;
                }
                let again = QwkMessage::read(encoded.as_slice(), extended)
                    .expect("an encoded message must parse again");

                // Text that starts like a kludge line is read back as one, which
                // the block padding can also turn a bare "Subject:" into.
                let text = message.text.as_slice();
                if extended
                    && (text.starts_with(b"To:")
                        || text.starts_with(b"From:")
                        || text.starts_with(b"Subject:"))
                {
                    continue;
                }
                // The fixed slots are padded with spaces, so trailing ones do not survive.
                assert_eq!(again.to.trim_end(), message.to.trim_end());
                assert_eq!(again.from.trim_end(), message.from.trim_end());
                assert_eq!(again.subj.trim_end(), message.subj.trim_end());
                assert_eq!(again.msg_number, message.msg_number);
                assert_eq!(again.conference_number, message.conference_number);
            }
        }
    }

    assert!(
        accepted > 500,
        "only {accepted} mutated messages were accepted"
    );
}

const CONTROL_DAT: &[u8] = b"My BBS\nNew York\n212-555\nJohn Doe\n20052,MYBBS\n\
01-01-1991,23:59:59\nJANE DOE\n\n0\n999\n1\n0\nMain Board\nHELLO\nNEWS\nSCRIPT0\n0\n2\n";

#[test]
fn test_fuzz_control_dat_round_trips_what_it_accepts() {
    let mut fuzzer = Fuzzer::new(0xC0FF_EE01);
    let seed = CONTROL_DAT.to_vec();
    let mut accepted = 0;

    for _ in 0..4000 {
        let data = fuzzer.mutate(&seed);
        let Ok(control) = ControlDat::read(&data) else {
            continue;
        };
        accepted += 1;

        let again =
            ControlDat::read(&control.to_vec()).expect("a generated CONTROL.DAT must parse again");
        // Lines are written with a plain newline, so a value ending in CR reads
        // back as if it had been the CR of a CRLF pair.
        assert_eq!(again.bbs_name.trim_end(), control.bbs_name.trim_end());
        assert_eq!(again.bbs_id.trim_end(), control.bbs_id.trim_end());
        assert_eq!(again.serial_number, control.serial_number);
        assert_eq!(again.message_count, control.message_count);
        assert_eq!(again.conferences.len(), control.conferences.len());
        assert_eq!(again.extra_lines.len(), control.extra_lines.len());
    }

    assert!(
        accepted > 500,
        "only {accepted} mutated CONTROL.DAT files were accepted"
    );
}

#[test]
fn test_fuzz_qwk_index_rejects_misaligned_data() {
    let seed = std::fs::read("data/qwk/000.ndx").unwrap();
    let mut fuzzer = Fuzzer::new(0x011D_5EED);

    for _ in 0..4000 {
        let data = fuzzer.mutate(&seed);
        match QwkMessageBase::convert_qwk_index(&data) {
            Ok(records) => assert_eq!(records.len(), data.len() / 5),
            Err(_) => continue,
        }
    }
}
