use bstr::BString;
use chrono::{TimeZone, Utc};
use jamjam::jam::pack::{PackOptions, PackReport};
use jamjam::jam::{JamMessage, JamMessageBase, attributes};
use jamjam::util::echomail::EchomailAddress;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

/// Four messages, "one" to "four", all written in 1990.
fn base(dir: &TempDir) -> JamMessageBase {
    let mut base = JamMessageBase::create(dir.path().join("packing")).unwrap();
    for (no, text) in ["one", "two", "three", "four"].iter().enumerate() {
        let msg = JamMessage::new(no as u32 + 1, &EchomailAddress::default())
            .with_date_time(Utc.with_ymd_and_hms(1990, 1, 1, 0, 0, 0).unwrap())
            .with_from(BString::from("sysop"))
            .with_to(BString::from(*text))
            .with_subject(BString::from(*text))
            .with_text(BString::from(*text));
        base.write_message(&msg).unwrap();
    }
    base.write_jhr_header().unwrap();
    base
}

fn texts(base: &JamMessageBase) -> Vec<String> {
    (base.lowest_message_number()..=base.highest_message_number())
        .filter_map(|no| base.read_header(no).ok())
        .map(|header| base.read_message_text(&header).unwrap().to_string())
        .collect()
}

#[test]
fn test_pack_removes_deleted_messages() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir);
    base.delete_message(2).unwrap();

    let report = base.pack(&PackOptions::default()).unwrap();

    assert_eq!(
        report,
        PackReport {
            before: 4,
            removed: 1,
            kept: 3,
            low_message_number: 1,
            high_message_number: 4
        }
    );
    assert_eq!(texts(&base), vec!["one", "three", "four"]);
}

/// Without renumbering the surviving messages keep the number they had, which
/// is what keeps the last-read pointers of the users valid.
#[test]
fn test_pack_keeps_message_numbers() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir);
    base.delete_message(2).unwrap();
    base.pack(&PackOptions::default()).unwrap();

    assert_eq!(base.read_header(3).unwrap().message_number, 3);
    assert!(base.read_header(2).is_err());

    // The gap must not confuse the next message written into the base.
    let msg = JamMessage::new(5, &EchomailAddress::default()).with_text(BString::from("five"));
    assert_eq!(base.write_message(&msg).unwrap(), 5);
    assert_eq!(base.read_header(5).unwrap().message_number, 5);
    assert_eq!(
        base.read_message_text(&base.read_header(1).unwrap())
            .unwrap(),
        "one"
    );
}

#[test]
fn test_pack_renumbers() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir);
    base.delete_message(1).unwrap();

    let report = base
        .pack(&PackOptions {
            renumber_from: Some(1),
            ..Default::default()
        })
        .unwrap();

    assert_eq!(
        report,
        PackReport {
            before: 4,
            removed: 1,
            kept: 3,
            low_message_number: 1,
            high_message_number: 3
        }
    );
    assert_eq!(texts(&base), vec!["two", "three", "four"]);
    assert_eq!(base.read_header(1).unwrap().message_number, 1);

    let msg = JamMessage::new(4, &EchomailAddress::default()).with_text(BString::from("five"));
    assert_eq!(base.write_message(&msg).unwrap(), 4);
}

#[test]
fn test_pack_renumbers_from_a_new_low_number() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir);

    base.pack(&PackOptions {
        renumber_from: Some(500),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(base.lowest_message_number(), 500);
    assert_eq!(base.highest_message_number(), 503);
    assert_eq!(base.active_messages(), 4);
    assert_eq!(
        base.read_message_text(&base.read_header(500).unwrap())
            .unwrap(),
        "one"
    );
    assert_eq!(base.read_header(503).unwrap().message_number, 503);
}

#[test]
fn test_pack_purges_by_date() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir);
    let msg = JamMessage::new(5, &EchomailAddress::default())
        .with_date_time(Utc.with_ymd_and_hms(1995, 1, 1, 0, 0, 0).unwrap())
        .with_text(BString::from("recent"));
    base.write_message(&msg).unwrap();

    let report = base
        .pack(&PackOptions {
            purge_before: Some(Utc.with_ymd_and_hms(1992, 1, 1, 0, 0, 0).unwrap()),
            ..Default::default()
        })
        .unwrap();

    assert_eq!(report.removed, 4);
    assert_eq!(texts(&base), vec!["recent"]);
}

#[test]
fn test_pack_purges_received_private_messages() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir);
    base.set_attributes(2, attributes::MSG_PRIVATE | attributes::MSG_READ, 0)
        .unwrap();
    // Unread private mail stays, it is still of use to the receiver.
    base.set_attributes(3, attributes::MSG_PRIVATE, 0).unwrap();

    let report = base
        .pack(&PackOptions {
            purge_received_private: true,
            ..Default::default()
        })
        .unwrap();

    assert_eq!(report.removed, 1);
    assert_eq!(texts(&base), vec!["one", "three", "four"]);
}

#[test]
fn test_pack_purges_messages_whose_packout_date_arrived() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir);
    let expired = JamMessage::new(5, &EchomailAddress::default())
        .with_packout_date(Utc.with_ymd_and_hms(1991, 1, 1, 0, 0, 0).unwrap())
        .with_text(BString::from("expired"));
    base.write_message(&expired).unwrap();
    let later = JamMessage::new(6, &EchomailAddress::default())
        .with_packout_date(Utc.with_ymd_and_hms(2999, 1, 1, 0, 0, 0).unwrap())
        .with_text(BString::from("later"));
    base.write_message(&later).unwrap();

    let report = base.pack(&PackOptions::default()).unwrap();

    assert_eq!(report.removed, 1);
    assert_eq!(texts(&base), vec!["one", "two", "three", "four", "later"]);
}

/// Packing has to reclaim the space of the removed messages, otherwise there
/// would be no point in running it.
#[test]
fn test_pack_shrinks_the_files() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir);
    let before = std::fs::metadata(dir.path().join("packing.jhr"))
        .unwrap()
        .len();
    base.delete_message(2).unwrap();
    base.pack(&PackOptions::default()).unwrap();

    assert!(
        std::fs::metadata(dir.path().join("packing.jhr"))
            .unwrap()
            .len()
            < before
    );
    assert_eq!(
        std::fs::metadata(dir.path().join("packing.jdt"))
            .unwrap()
            .len(),
        "onethreefour".len() as u64
    );
}

#[test]
fn test_reindex_rebuilds_a_lost_index() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir);
    // A header that grew was appended, so the index has to find the last copy.
    let mut header = base.read_header(2).unwrap();
    header.set_subject(BString::from("a much longer subject than before"));
    base.update_header(2, &header).unwrap();
    std::fs::write(dir.path().join("packing.jdx"), []).unwrap();

    let report = base
        .pack(&PackOptions {
            index_only: true,
            ..Default::default()
        })
        .unwrap();

    assert_eq!(report.kept, 4);
    assert_eq!(report.removed, 0);
    assert_eq!(base.active_messages(), 4);
    assert_eq!(texts(&base), vec!["one", "two", "three", "four"]);
    assert_eq!(
        base.read_header(2).unwrap().subject().unwrap(),
        "a much longer subject than before"
    );
}

/// Rebuilding the index must not throw away messages the sysop can still recover.
#[test]
fn test_reindex_keeps_deleted_messages() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir);
    base.delete_message(2).unwrap();
    std::fs::write(dir.path().join("packing.jdx"), []).unwrap();

    base.pack(&PackOptions {
        index_only: true,
        ..Default::default()
    })
    .unwrap();
    base.restore_message(2).unwrap();

    assert_eq!(texts(&base), vec!["one", "two", "three", "four"]);
}
