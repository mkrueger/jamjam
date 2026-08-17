use bstr::BString;
use jamjam::jam::msg_header::{JamMessageHeader, SubfieldType};
use jamjam::jam::pack::PackOptions;
use jamjam::jam::{JamMessage, JamMessageBase};
use jamjam::util::echomail::EchomailAddress;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn message(text: &str, to: &str) -> JamMessage {
    JamMessage::new(0, &EchomailAddress::default())
        .with_from(BString::from("sysop"))
        .with_to(BString::from(to))
        .with_subject(BString::from(text))
        .with_text(BString::from(text))
}

#[test]
fn test_open_base() {
    let base = JamMessageBase::open("data/jam/general").unwrap();

    assert_eq!(base.active_messages(), 4);
    assert_eq!(base.lowest_message_number(), 1);
    assert_eq!(base.mod_counter(), 4);
    assert!(!base.needs_password());
}

#[test]
fn test_read_headers() {
    let base = JamMessageBase::open("data/jam/general").unwrap();
    let headers = base.read_headers().unwrap();
    assert_eq!(base.active_messages(), headers.len() as u32);
}

#[test]
fn test_get_header() {
    let base = JamMessageBase::open("data/jam/general").unwrap();
    let header = base.read_header(3).unwrap();
    assert_eq!(header.from().unwrap(), "omnibrain");
    assert_eq!(header.subject().unwrap(), "Re: Hello All");
    assert_eq!(header.reply_to, 2);
}

#[test]
fn test_get_text() {
    let base = JamMessageBase::open("data/jam/general").unwrap();
    let header = base.read_header(4).unwrap();
    let txt = base.read_message_text(&header).unwrap();
    assert_eq!(
        txt,
        "private message\r\r... Multitasking: Reading in the bathroom\r"
    );
}

#[test]
fn test_iter_ra() {
    let base = JamMessageBase::open("data/jam/ra").unwrap();
    for msg in base.iter().unwrap() {
        let msg = msg.unwrap();
        for sub in msg.sub_fields.iter() {
            if sub.field_type() == SubfieldType::MsgID {
                assert_eq!(JamMessageBase::crc(sub.content()), msg.msgid_crc);
            }
        }
    }
    assert_eq!(3, base.iter().unwrap().count());
}

#[test]
fn test_iter_mystic() {
    let base = JamMessageBase::open("data/jam/general").unwrap();
    for msg in base.iter().unwrap() {
        let msg = msg.unwrap();
        for sub in msg.sub_fields.iter() {
            if sub.field_type() == SubfieldType::MsgID {
                assert_eq!(JamMessageBase::crc(sub.content()), msg.msgid_crc);
            }
        }
    }
    assert_eq!(4, base.iter().unwrap().count());
}

#[test]
fn test_iter_elebbs() {
    let base = JamMessageBase::open("data/jam/elebbs").unwrap();
    for msg in base.iter().unwrap() {
        let msg = msg.unwrap();
        for sub in msg.sub_fields.iter() {
            if sub.field_type() == SubfieldType::MsgID {
                assert_eq!(JamMessageBase::crc(sub.content()), msg.msgid_crc);
            }
        }
    }
    assert_eq!(4, base.iter().unwrap().count());
}

/// `ActiveMsgs` counts the messages that are not deleted, it is not the highest
/// message number. The next number has to come from the index instead.
#[test]
fn test_active_count_is_separate_from_the_highest_number() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("numbers");
    let mut base = JamMessageBase::create(&path).unwrap();
    for text in ["one", "two", "three"] {
        base.write_message(&message(text, "all")).unwrap();
    }

    assert_eq!(base.active_messages(), 3);
    assert_eq!(base.lowest_message_number(), 1);
    assert_eq!(base.highest_message_number(), 3);

    base.delete_message(2).unwrap();
    assert_eq!(base.active_messages(), 2);
    assert_eq!(base.highest_message_number(), 3);

    // The gap must not hand out a number that is already taken.
    assert_eq!(base.write_message(&message("four", "all")).unwrap(), 4);
    assert_eq!(base.active_messages(), 3);

    let reopened = JamMessageBase::open(&path).unwrap();
    assert_eq!(reopened.active_messages(), 3);
    assert_eq!(reopened.highest_message_number(), 4);
    assert_eq!(
        reopened
            .read_message_text(&reopened.read_header(4).unwrap())
            .unwrap(),
        "four"
    );
}

#[test]
fn test_restore_brings_the_active_count_back() {
    let dir = TempDir::new().unwrap();
    let mut base = JamMessageBase::create(dir.path().join("restore")).unwrap();
    base.write_message(&message("one", "all")).unwrap();
    base.delete_message(1).unwrap();
    assert_eq!(base.active_messages(), 0);
    base.restore_message(1).unwrap();
    assert_eq!(base.active_messages(), 1);
    // Restoring twice must not count the message again.
    base.restore_message(1).unwrap();
    assert_eq!(base.active_messages(), 1);
}

/// The second ulong of an index record is a header offset, the message number
/// is the record number plus BaseMsgNum.
#[test]
fn test_search_index_returns_message_numbers() {
    let dir = TempDir::new().unwrap();
    let mut base = JamMessageBase::create(dir.path().join("search")).unwrap();
    for to in ["alice", "bob", "alice"] {
        base.write_message(&message(to, to)).unwrap();
    }

    let crc = JamMessageBase::crc(&BString::from("alice"));
    assert_eq!(base.search_message_index(crc).unwrap(), vec![1, 3]);

    let crc = JamMessageBase::crc(&BString::from("bob"));
    assert_eq!(base.search_message_index(crc).unwrap(), vec![2]);
}

#[test]
fn test_search_index_follows_the_base_message_number() {
    let dir = TempDir::new().unwrap();
    let mut base = JamMessageBase::create(dir.path().join("search")).unwrap();
    for to in ["alice", "bob"] {
        base.write_message(&message(to, to)).unwrap();
    }
    base.pack(&PackOptions {
        renumber_from: Some(500),
        ..Default::default()
    })
    .unwrap();

    let crc = JamMessageBase::crc(&BString::from("bob"));
    assert_eq!(base.search_message_index(crc).unwrap(), vec![501]);
}

#[test]
fn test_header_round_trip() {
    let mut header = JamMessageHeader {
        message_number: 42,
        times_read: 7,
        ..Default::default()
    };
    header.set_to(BString::from("alice"));
    header.set_from(BString::from("bob"));
    header.set_subject(BString::from("hello"));

    let mut buf = Vec::new();
    header.write(&mut buf).unwrap();
    let read = JamMessageHeader::read(&mut buf.as_slice()).unwrap();

    assert_eq!(read.message_number, 42);
    assert_eq!(read.times_read, 7);
    assert_eq!(read.to().unwrap(), "alice");
    assert_eq!(read.from().unwrap(), "bob");
    assert_eq!(read.subject().unwrap(), "hello");
}

#[test]
fn test_truncated_header_is_an_error() {
    let mut header = JamMessageHeader::default();
    header.set_subject(BString::from("a subject long enough to cut"));
    let mut buf = Vec::new();
    header.write(&mut buf).unwrap();
    buf.truncate(buf.len() - 4);

    assert!(JamMessageHeader::read(&mut buf.as_slice()).is_err());
}

#[test]
fn test_bad_signature_is_an_error() {
    let header = JamMessageHeader::default();
    let mut buf = Vec::new();
    header.write(&mut buf).unwrap();
    buf[0] = b'X';

    assert!(JamMessageHeader::read(&mut buf.as_slice()).is_err());
}

#[test]
fn test_misaligned_index_is_rejected() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("broken");
    let mut base = JamMessageBase::create(&path).unwrap();
    base.write_message(&message("one", "all")).unwrap();

    std::fs::write(path.with_extension("jdx"), [0u8; 5]).unwrap();
    assert!(JamMessageBase::open(&path).is_err());
}

/// A replaced header is appended, the old record has to be marked deleted with
/// a zero text length so packing does not free the text of the new record.
#[test]
fn test_update_header_retires_the_old_record() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("update");
    let mut base = JamMessageBase::create(&path).unwrap();
    base.write_message(&message("one", "all")).unwrap();

    let mut header = base.read_header(1).unwrap();
    header.set_subject(BString::from("a much longer subject than before"));
    base.update_header(1, &header).unwrap();

    assert_eq!(
        base.read_header(1).unwrap().subject().unwrap(),
        "a much longer subject than before"
    );
    assert_eq!(
        base.read_message_text(&base.read_header(1).unwrap())
            .unwrap(),
        "one"
    );

    let retired = base.iter().unwrap().next().unwrap().unwrap();
    assert!(retired.is_deleted());
    assert_eq!(retired.txt_len, 0);
}
