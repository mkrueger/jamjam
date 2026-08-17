use bstr::BString;
use jamjam::conversion::{convert_pcboard_to_jam, convert_qwk_to_jam};
use jamjam::jam::JamMessageBase;
use jamjam::pcboard::PCBoardMessageBase;
use jamjam::qwk::QwkMessageBase;
use jamjam::qwk::qwk_message::QwkMessage;
use jamjam::util::cp437::to_utf8;
use jamjam::util::echomail::EchomailAddress;
use pretty_assertions::assert_eq;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_pcboard_to_jam_keeps_the_fields() {
    let tmpdir = TempDir::with_prefix_in("jamtest", ".").unwrap();
    let converted = tmpdir.path().join("jambase");
    convert_pcboard_to_jam(
        &PathBuf::from("data/pcboard/test"),
        &converted,
        &EchomailAddress::default(),
    )
    .unwrap();

    let pcb = PCBoardMessageBase::open("data/pcboard/test").unwrap();
    let jam = JamMessageBase::open(&converted).unwrap();

    assert_eq!(pcb.active_messages(), jam.active_messages());

    for i in 1..=pcb.active_messages() {
        let pcb_msg = pcb.read_message(i).unwrap();
        let jam_msg = jam.read_header(i).unwrap();
        let jam_txt = jam.read_message_text(&jam_msg).unwrap();

        assert!(pcb_msg.header.msg_number == jam_msg.message_number);
        assert!(pcb_msg.header.to_field == *jam_msg.to().unwrap());
        assert!(pcb_msg.header.from_field == *jam_msg.from().unwrap());
        assert!(pcb_msg.header.subj_field == *jam_msg.subject().unwrap());
        assert!(pcb_msg.text == jam_txt);
    }
}

fn qwk_mail() -> Vec<QwkMessage> {
    let msg_base = QwkMessageBase::open("data/qwk", true).unwrap();
    msg_base
        .iter()
        .unwrap()
        .collect::<jamjam::Result<_>>()
        .unwrap()
}

#[test]
fn test_qwk_conversion_keeps_the_message_text() {
    let dir = TempDir::new().unwrap();
    let mail = qwk_mail();
    let mut jam = JamMessageBase::create(dir.path().join("converted")).unwrap();
    convert_qwk_to_jam(&mail, &mut jam).unwrap();

    assert_eq!(jam.active_messages(), mail.len() as u32);
    for (i, msg) in mail.iter().enumerate() {
        let header = jam.read_header(i as u32 + 1).unwrap();
        assert_eq!(*header.from().unwrap(), BString::from(to_utf8(&msg.from)));
        assert_eq!(*header.to().unwrap(), BString::from(to_utf8(&msg.to)));
        assert_eq!(
            *header.subject().unwrap(),
            BString::from(to_utf8(&msg.subj))
        );
        assert_eq!(
            jam.read_message_text(&header).unwrap(),
            BString::from(to_utf8(&msg.text))
        );
    }
}

#[test]
fn test_qwk_conversion_numbers_messages_from_the_base() {
    let dir = TempDir::new().unwrap();
    let mail = qwk_mail();
    let mut jam = JamMessageBase::create(dir.path().join("converted")).unwrap();
    convert_qwk_to_jam(&mail, &mut jam).unwrap();

    assert_eq!(jam.lowest_message_number(), 1);
    assert_eq!(jam.highest_message_number(), mail.len() as u32);
}
