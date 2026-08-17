use super::{
    QwkMessageBase,
    control::{Conference, ControlDat},
    qwk_message::{MSG_ACTIVE, QwkMessage},
};
use bstr::BString;
use pretty_assertions::assert_eq;

const TEST_CONTROL_DAT: &[u8; 114] = b"My BBS
New York
212-555
John Doe
20052,MYBBS
01-01-1991,23:59:59
JANE DOE

0
999
1
0
Main Board
HELLO
NEWS
SCRIPT0";

#[test]
fn test_parse_control_dat() {
    let dat = ControlDat::read(TEST_CONTROL_DAT).unwrap();
    assert_eq!(dat.bbs_name, "My BBS");
    assert_eq!(dat.bbs_city_and_state, "New York");
    assert_eq!(dat.bbs_phone_number, "212-555");
    assert_eq!(dat.bbs_sysop_name, "John Doe");
    assert_eq!(dat.bbs_id, "MYBBS");
    assert_eq!(dat.serial_number, 20052);
    assert_eq!(dat.creation_time, "01-01-1991,23:59:59");
    assert_eq!(dat.qmail_user_name, "JANE DOE");
    assert_eq!(dat.qmail_menu_name, "");
    assert_eq!(dat.message_count, 999);
    assert_eq!(dat.conferences.len(), 1);
    assert_eq!(dat.conferences[0].number, 0);
    assert_eq!(dat.conferences[0].name, "Main Board");
    assert_eq!(dat.welcome_screen, "HELLO");
    assert_eq!(dat.news_screen, "NEWS");
    assert_eq!(dat.logoff_screen, "SCRIPT0");
}

#[test]
fn test_generate_control_dat() {
    let dat = ControlDat {
        bbs_name: BString::from("My BBS"),
        bbs_city_and_state: BString::from("New York"),
        bbs_phone_number: BString::from("212-555"),
        bbs_sysop_name: BString::from("John Doe"),
        bbs_id: BString::from("MYBBS"),
        creation_time: BString::from("01-01-1991,23:59:59"),
        qmail_user_name: BString::from("JANE DOE"),
        qmail_menu_name: BString::from(""),
        zero_line: BString::from("0"),
        message_count: 999,
        serial_number: 20052,
        conferences: vec![Conference {
            number: 0,
            name: BString::from("Main Board"),
        }],
        welcome_screen: BString::from("HELLO"),
        news_screen: BString::from("NEWS"),
        logoff_screen: BString::from("SCRIPT0"),
    };

    assert_eq!(dat.to_vec(), TEST_CONTROL_DAT);
}

#[test]
fn read_qwk_repo() {
    let msg_base = QwkMessageBase::open("data/qwk", true).unwrap();

    // CP437 encoding fun - that's why the bstr crate is great :).
    assert_eq!(
        msg_base.bbs_name().to_vec(),
        b"\xAE\xAE PCBoard Professional Bulletin Board \xAF\xAF".to_vec()
    );
    assert_eq!(msg_base.bbs_sysop_name(), "Sysop, Sysop");

    let mail = msg_base.read_conference_mail(0).unwrap();
    assert_eq!(mail.len(), 1);
    assert_eq!(mail[0].from, "SYSOP");
    assert_eq!(mail[0].to, "ALL");
    assert_eq!(mail[0].subj, "test");
}

#[test]
fn test_index() {
    let in_data = vec![
        0x00, 0x00, 0x28, 0x87, 0x19, 0x00, 0x00, 0x30, 0x87, 0x19, 0x00, 0x00, 0x38, 0x87, 0x19,
        0x00, 0x00, 0x7E, 0x87, 0x19, 0x00, 0x00, 0x07, 0x88, 0x19, 0x00, 0x00, 0x0B, 0x88, 0x19,
        0x00, 0x00, 0x0F, 0x88, 0x19, 0x00, 0x00, 0x14, 0x88, 0x19, 0x00, 0x00, 0x19, 0x88, 0x19,
        0x00, 0x00, 0x1E, 0x88, 0x19, 0x00, 0x00, 0x22, 0x88, 0x19, 0x00, 0x00, 0x27, 0x88, 0x19,
        0x00, 0x00, 0x2C, 0x88, 0x19, 0x00, 0x00, 0x31, 0x88, 0x19, 0x00, 0x00, 0x3B, 0x88, 0x19,
        0x00, 0x00, 0x40, 0x88, 0x19, 0x00, 0x00, 0x46, 0x88, 0x19, 0x00, 0x00, 0x49, 0x88, 0x19,
        0x00, 0x00, 0x4D, 0x88, 0x19, 0x00, 0x00, 0x52, 0x88, 0x19, 0x00, 0x00, 0x55, 0x88, 0x19,
        0x00, 0x00, 0x59, 0x88, 0x19, 0x00, 0x00, 0x60, 0x88, 0x19, 0x00, 0x00, 0x66, 0x88, 0x19,
        0x00, 0x00, 0x70, 0x88, 0x19,
    ];
    let out_data = vec![
        84, 88, 92, 127, 135, 139, 143, 148, 153, 158, 162, 167, 172, 177, 187, 192, 198, 201, 205,
        210, 213, 217, 224, 230, 240,
    ];

    let res = QwkMessageBase::convert_qwk_index(&in_data).unwrap();
    assert_eq!(res, out_data);
}

fn sample_message(to: &str) -> QwkMessage {
    QwkMessage {
        status: b' ',
        msg_number: 12,
        date_time: BString::from("01-01-9112:30"),
        to: BString::from(to),
        from: BString::from("SYSOP"),
        subj: BString::from("test"),
        password: BString::from(""),
        ref_msg_number: 0,
        active_flag: MSG_ACTIVE,
        conference_number: 0,
        logical_message_number: 1,
        net_tag: b' ',
        text: BString::from("hello\n"),
    }
}

/// A field wider than its slot used to push the header past 128 bytes, which
/// shifts every block that follows it in the packet.
#[test]
fn test_long_fields_do_not_grow_the_header() {
    let long = "A VERY LONG RECIPIENT NAME THAT EXCEEDS THE FIELD";
    let mut buf = Vec::new();
    let blocks = sample_message(long).write(&mut buf, true).unwrap();

    assert_eq!(buf.len(), blocks * QwkMessage::HEADER_SIZE);
    assert_eq!(&buf[21..46], &long.as_bytes()[..25]);
}

#[test]
fn test_extended_round_trip_restores_long_fields() {
    let long = "A VERY LONG RECIPIENT NAME THAT EXCEEDS THE FIELD";
    let mut buf = Vec::new();
    sample_message(long).write(&mut buf, true).unwrap();

    let read = QwkMessage::read(buf.as_slice(), true).unwrap();
    assert_eq!(read.to, long);
    assert_eq!(read.from, "SYSOP");
    assert_eq!(read.subj, "test");
    assert_eq!(read.msg_number, 12);
    assert!(read.text.to_string().contains("hello"));
}

#[test]
fn test_round_trip_of_a_short_message() {
    let mut buf = Vec::new();
    sample_message("ALL").write(&mut buf, false).unwrap();

    let read = QwkMessage::read(buf.as_slice(), false).unwrap();
    assert_eq!(read.to, "ALL");
    assert_eq!(read.date_time, "01-01-9112:30");
    assert_eq!(read.date_time().to_string(), "1991-01-01 12:30:00");
}

#[test]
fn test_number_too_wide_for_its_field_is_rejected() {
    let mut msg = sample_message("ALL");
    msg.msg_number = 99_999_999;
    assert!(msg.write(Vec::new(), false).is_err());
}

#[test]
fn test_misaligned_index_is_rejected() {
    assert!(QwkMessageBase::convert_qwk_index(&[0, 0, 0, 0]).is_err());
    assert!(QwkMessageBase::convert_qwk_index(&[0, 0, 0, 0, 0, 1]).is_err());
}

#[test]
fn test_truncated_message_is_rejected() {
    let mut buf = Vec::new();
    sample_message("ALL").write(&mut buf, false).unwrap();
    buf.truncate(buf.len() - 1);

    assert!(QwkMessage::read(buf.as_slice(), false).is_err());
}
