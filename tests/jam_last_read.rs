use bstr::BString;
use jamjam::jam::last_read_storage::JamLastReadStorage;
use jamjam::jam::msg_header::JamMessageHeader;
use jamjam::jam::{JamMessage, JamMessageBase};
use jamjam::util::echomail::EchomailAddress;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn base(dir: &TempDir, name: &str) -> JamMessageBase {
    JamMessageBase::create(dir.path().join(name)).unwrap()
}

fn storage(user_crc: u32, user_id: u32, last_read: u32) -> JamLastReadStorage {
    JamLastReadStorage {
        user_crc,
        user_id,
        last_read_msg: last_read,
        high_read_msg: last_read,
    }
}

/// Storing without looking the user up first used to seek to a wrapped offset.
#[test]
fn test_write_last_read_without_a_previous_lookup() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir, "lr");

    base.write_last_read(&storage(111, 222, 7)).unwrap();

    let found = base.find_last_read(111, 222).unwrap().unwrap();
    assert_eq!(found.last_read_msg, 7);
    assert_eq!(base.read_last_read_file().unwrap().len(), 1);
}

/// A record may sit anywhere in the file, so storing has to find it again
/// instead of trusting a remembered position.
#[test]
fn test_write_last_read_updates_in_place() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir, "lr");
    base.write_last_read(&storage(1, 1, 10)).unwrap();
    base.write_last_read(&storage(2, 2, 20)).unwrap();
    base.write_last_read(&storage(3, 3, 30)).unwrap();

    base.write_last_read(&storage(2, 2, 99)).unwrap();

    let all = base.read_last_read_file().unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(
        base.find_last_read(1, 1).unwrap().unwrap().last_read_msg,
        10
    );
    assert_eq!(
        base.find_last_read(2, 2).unwrap().unwrap().last_read_msg,
        99
    );
    assert_eq!(
        base.find_last_read(3, 3).unwrap().unwrap().last_read_msg,
        30
    );
}

/// Two users interleaved on one base must not overwrite each other.
#[test]
fn test_interleaved_users_keep_their_own_record() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir, "lr");
    base.create_last_read(10, 1).unwrap();
    base.create_last_read(20, 2).unwrap();

    let mut first = base.find_last_read(10, 1).unwrap().unwrap();
    let mut second = base.find_last_read(20, 2).unwrap().unwrap();
    first.last_read_msg = 5;
    second.last_read_msg = 6;
    base.write_last_read(&first).unwrap();
    base.write_last_read(&second).unwrap();

    assert_eq!(
        base.find_last_read(10, 1).unwrap().unwrap().last_read_msg,
        5
    );
    assert_eq!(
        base.find_last_read(20, 2).unwrap().unwrap().last_read_msg,
        6
    );
    assert_eq!(base.read_last_read_file().unwrap().len(), 2);
}

#[test]
fn test_create_last_read_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir, "lr");
    base.create_last_read(7, 8).unwrap();
    base.write_last_read(&storage(7, 8, 33)).unwrap();

    let again = base.create_last_read(7, 8).unwrap();
    assert_eq!(again.last_read_msg, 33);
    assert_eq!(base.read_last_read_file().unwrap().len(), 1);
}

#[test]
fn test_missing_user_is_none() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir, "lr");
    base.create_last_read(1, 1).unwrap();

    assert!(base.find_last_read(2, 2).unwrap().is_none());
}

#[test]
fn test_deleted_record_is_not_found() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir, "lr");
    base.write_last_read(&storage(1, 1, 5)).unwrap();

    assert!(base.delete_last_read(1, 1).unwrap());
    assert!(base.find_last_read(1, 1).unwrap().is_none());
    assert!(!base.delete_last_read(1, 1).unwrap());

    // The slot stays, a new record for the user is appended after it.
    base.write_last_read(&storage(1, 1, 9)).unwrap();
    assert_eq!(base.find_last_read(1, 1).unwrap().unwrap().last_read_msg, 9);
}

#[test]
fn test_truncated_lastread_file_is_rejected() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("lr");
    let mut base = JamMessageBase::create(&path).unwrap();
    base.write_last_read(&storage(1, 1, 1)).unwrap();

    std::fs::write(path.with_extension("jlr"), [0u8; 20]).unwrap();
    assert!(base.read_last_read_file().is_err());
    assert!(base.find_last_read(1, 1).is_err());
}

/// A header claiming more text than the .JDT holds must not drive an allocation.
#[test]
fn test_text_running_past_the_file_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir, "text");
    let msg = JamMessage::new(&EchomailAddress::default())
        .with_to(BString::from("all"))
        .with_text(BString::from("short"));
    base.write_message(&msg).unwrap();

    let mut header = base.read_header(1).unwrap();
    assert_eq!(base.read_message_text(&header).unwrap(), "short");

    header.txt_len = u32::MAX;
    assert!(base.read_message_text(&header).is_err());

    header.txt_len = 5;
    header.offset = u32::MAX - 4;
    assert!(base.read_message_text(&header).is_err());
}

#[test]
fn test_header_round_trip_survives_a_zero_length_message() {
    let dir = TempDir::new().unwrap();
    let mut base = base(&dir, "empty");
    let msg = JamMessage::new(&EchomailAddress::default()).with_to(BString::from("all"));
    base.write_message(&msg).unwrap();

    let header: JamMessageHeader = base.read_header(1).unwrap();
    assert_eq!(header.txt_len, 0);
    assert_eq!(base.read_message_text(&header).unwrap(), "");
}
