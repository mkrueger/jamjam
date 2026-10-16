use jamjam::pcboard::{MessageStatus, PCBoardMessage, PCBoardMessageBase};
use pretty_assertions::assert_eq;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_open_base() {
    let mut base = PCBoardMessageBase::open("data/pcboard/test").unwrap();
    assert!(!base.is_locked().unwrap());

    assert_eq!(base.active_messages(), 4);
    assert_eq!(base.highest_message_number(), 4);
    assert_eq!(base.lowest_message_number(), 1);
    assert_eq!(base.callers(), 1);
}

#[test]
fn test_old_index() {
    let base = PCBoardMessageBase::open("data/pcboard/test").unwrap();
    let idx = base.read_old_index().unwrap();
    assert_eq!(idx.len(), 4);
}

#[test]
fn test_index() {
    let base = PCBoardMessageBase::open("data/pcboard/test").unwrap();

    let old_idx = base.read_old_index().unwrap();
    let new_idx = base.read_index().unwrap();
    assert_eq!(old_idx.len(), new_idx.len());
    for i in 0..old_idx.len() {
        assert_eq!(old_idx[i], new_idx[i].offset as u32);
    }
}

#[test]
fn test_read_message() {
    let base = PCBoardMessageBase::open("data/pcboard/test").unwrap();
    let msg = base.read_message(3).unwrap();
    assert_eq!(msg.header.from_field, "SYSOP");
    assert_eq!(msg.header.to_field, "ALL");
    assert_eq!(msg.header.subj_field, "Another message");
    assert!(!msg.header.password.is_empty());
}

#[test]
fn test_iter() {
    let base = PCBoardMessageBase::open("data/pcboard/test").unwrap();
    for (i, msg) in base.iter().unwrap().enumerate() {
        let msg = msg.unwrap();
        match i {
            0 => {
                assert_eq!(MessageStatus::SenderPassword, msg.status());
                assert_eq!("SECRET", msg.header.password);
            }
            1 => {
                assert_eq!(MessageStatus::Public, msg.status());
            }
            2 => {
                assert_eq!(MessageStatus::GroupPasswordMessageToAll, msg.status());
                assert_eq!("GROUPPW", msg.header.password);
            }
            3 => {
                assert_eq!(MessageStatus::Public, msg.status());
            }
            _ => panic!("Too many messages"),
        }
    }
    assert_eq!(4, base.iter().unwrap().count());
}

#[test]
fn test_truncated_message_is_rejected() {
    let data = std::fs::read("data/pcboard/test").unwrap();
    // The first 128 byte block holds the message base header.
    let full = PCBoardMessage::read(&mut &data[128..]).unwrap();
    let total = 128 * full.header.num_blocks as usize;

    assert!(PCBoardMessage::read(&mut &data[128..128 + total - 1]).is_err());
}

fn copy_base(dir: &TempDir) -> PathBuf {
    let path = dir.path().join("test");
    std::fs::copy("data/pcboard/test", &path).unwrap();
    std::fs::copy("data/pcboard/test.idx", path.with_extension("idx")).unwrap();
    path
}

#[test]
fn test_lock_keeps_the_base_header() {
    let dir = TempDir::new().unwrap();
    let path = copy_base(&dir);
    let original = std::fs::read(&path).unwrap();

    let mut base = PCBoardMessageBase::open(&path).unwrap();
    base.lock().unwrap();

    let mut reopened = PCBoardMessageBase::open(&path).unwrap();
    assert!(reopened.is_locked().unwrap());
    assert_eq!(reopened.lowest_message_number(), 1);
    assert_eq!(reopened.highest_message_number(), 4);
    assert_eq!(reopened.active_messages(), 4);
    assert_eq!(reopened.callers(), 1);
    assert_eq!(reopened.read_message(3).unwrap().header.from_field, "SYSOP");

    reopened.unlock().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn test_truncated_index_is_rejected() {
    let dir = TempDir::new().unwrap();
    let path = copy_base(&dir);
    let idx_path = path.with_extension("idx");
    let mut idx = std::fs::read(&idx_path).unwrap();
    idx.pop();
    std::fs::write(&idx_path, idx).unwrap();

    let base = PCBoardMessageBase::open(&path).unwrap();
    assert!(base.read_index().is_err());
}
