use super::*;

#[test]
fn test_open_base() {
    let base = MessageBase::open("data/jam/general").unwrap();

    assert_eq!(base.active_messages(), 4);
    assert_eq!(base.base_messagenumber(), 1);
    assert_eq!(base.mod_counter(), 4);
    assert!(!base.needs_password());
}

#[test]
fn test_read_headers() {
    let base = MessageBase::open("data/jam/general").unwrap();
    let headers = base.read_headers().unwrap();
    assert_eq!(base.active_messages(), headers.len() as u32);
}

#[test]
fn test_get_header() {
    let base = MessageBase::open("data/jam/general").unwrap();
    let header = base.get_header(3).unwrap();
    assert_eq!(header.get_from().unwrap(), "omnibrain");
    assert_eq!(header.get_subject().unwrap(), "Re: Hello All");
    assert_eq!(header.reply_to, 2);
}

#[test]
fn test_get_text() {
    let base = MessageBase::open("data/jam/general").unwrap();
    let header = base.get_header(4).unwrap();
    let txt = base.get_msg_text(&header).unwrap();
    assert_eq!(
        txt,
        "private message\r\n\r\n... Multitasking: Reading in the bathroom\r\n"
    );
}
