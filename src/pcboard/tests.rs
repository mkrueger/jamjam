use super::*;

#[test]
fn test_open_base() {
    let base = MessageBase::open("data/pcboard/test").unwrap();
    assert_eq!(base.active_messages(), 4);
    assert_eq!(base.highest_message_number(), 4);
    assert_eq!(base.lowest_message_number(), 1);
    assert_eq!(base.callers(), 1);
}

#[test]
fn test_old_index() {
    let base = MessageBase::open("data/pcboard/test").unwrap();
    let idx = base.read_old_index().unwrap();
    assert_eq!(idx.len(), 4);
}

#[test]
fn test_index() {
    let base = MessageBase::open("data/pcboard/test").unwrap();

    let old_idx = base.read_old_index().unwrap();
    let new_idx = base.read_index().unwrap();
    assert_eq!(old_idx.len(), new_idx.len());
    for i in 0..old_idx.len() {
        assert_eq!(old_idx[i], new_idx[i].offset);
    }
}

#[test]
fn test_read_message() {
    let base = MessageBase::open("data/pcboard/test").unwrap();
    let msg = base.read_message(3).unwrap();
    assert_eq!(msg.header.from_field, "SYSOP");
    assert_eq!(msg.header.to_field, "ALL");
    assert_eq!(msg.header.subj_field, "Another message");
    assert!(!msg.header.password.is_empty());
}
