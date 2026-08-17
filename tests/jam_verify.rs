use bstr::BString;
use jamjam::jam::last_read_storage::JamLastReadStorage;
use jamjam::jam::verify::Problem;
use jamjam::jam::{JamMessage, JamMessageBase};
use jamjam::util::echomail::EchomailAddress;
use pretty_assertions::assert_eq;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use tempfile::TempDir;

fn message(text: &str) -> JamMessage {
    JamMessage::new(&EchomailAddress::default())
        .with_to(BString::from("all"))
        .with_subject(BString::from(text))
        .with_text(BString::from(text))
}

fn filled(path: &Path) -> JamMessageBase {
    let mut base = JamMessageBase::create(path).unwrap();
    for text in ["one", "two", "three"] {
        base.write_message(&message(text)).unwrap();
    }
    base
}

#[test]
fn test_verify_accepts_a_healthy_base() {
    let dir = TempDir::new().unwrap();
    let mut base = filled(&dir.path().join("ok"));

    let report = base.verify().unwrap();
    assert!(report.is_ok(), "{report}");
    assert_eq!(report.live_messages, 3);
    assert_eq!(report.index_records, 3);
}

#[test]
fn test_verify_reports_active_message_drift() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("drift");
    let mut base = filled(&path);
    base.delete_message(2).unwrap();

    let header = path.with_extension("jhr");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(header)
        .unwrap();
    file.seek(SeekFrom::Start(12)).unwrap();
    file.write_all(&99u32.to_le_bytes()).unwrap();
    drop(file);

    let mut base = JamMessageBase::open(&path).unwrap();
    let report = base.verify().unwrap();
    assert!(
        report.problems.iter().any(|problem| matches!(
            problem,
            Problem::ActiveMessageCountWrong {
                stored: 99,
                counted: 2
            }
        )),
        "{report:?}"
    );
}

#[test]
fn test_repair_rebuilds_a_lost_index_and_clamps_lastread() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("repair");
    let mut base = filled(&path);
    base.write_last_read(&JamLastReadStorage {
        user_crc: 1,
        user_id: 2,
        last_read_msg: 3,
        high_read_msg: 99,
    })
    .unwrap();
    drop(base);

    std::fs::write(path.with_extension("jdx"), []).unwrap();

    let mut base = JamMessageBase::open(&path).unwrap();
    let report = base.repair().unwrap();
    assert!(report.reindexed);
    assert_eq!(report.last_read_records_clamped, 1);
    assert!(
        report.after.is_ok()
            || report
                .after
                .problems
                .iter()
                .all(|problem| matches!(problem, Problem::UnreferencedText { .. })),
        "{:?}",
        report.after
    );
    assert_eq!(base.active_messages(), 3);
    assert_eq!(base.highest_message_number(), 3);
    let last = base.find_last_read(1, 2).unwrap().unwrap();
    assert_eq!(last.last_read_msg, 3);
    assert_eq!(last.high_read_msg, 3);
}

#[test]
fn test_verify_does_not_rewrite_the_base() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("readonly");
    let mut base = filled(&path);
    let before = std::fs::read(path.with_extension("jdx")).unwrap();
    let _ = base.verify().unwrap();
    assert_eq!(std::fs::read(path.with_extension("jdx")).unwrap(), before);
}
