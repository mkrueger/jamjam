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

#[test]
fn test_pcboard_to_jam_keeps_attachments() {
    use jamjam::jam::msg_header::SubfieldType;

    let dir = TempDir::new().unwrap();
    // Base header plus the first message header, turned into a two block
    // message whose body is a single ATTACH extended header.
    let mut data = std::fs::read("data/pcboard/test").unwrap()[..256].to_vec();
    data[128 + 9] = 2; // number of blocks
    data[128 + 126] = 16; // extended status: ATTACH
    let mut body = [b' '; 128];
    body[..10].copy_from_slice(b"\xff@ATTACH :");
    body[10..18].copy_from_slice(b"FILE.ZIP");
    body[70] = b'N';
    data.extend(body);
    let source = dir.path().join("attached");
    std::fs::write(&source, data).unwrap();

    let converted = dir.path().join("jambase");
    convert_pcboard_to_jam(&source, &converted, &EchomailAddress::default()).unwrap();

    let jam = JamMessageBase::open(&converted).unwrap();
    let header = jam.read_header(1).unwrap();
    let attachments: Vec<_> = header
        .sub_fields
        .iter()
        .filter(|field| field.field_type() == SubfieldType::EnclFile)
        .map(|field| field.content().clone())
        .collect();
    assert_eq!(attachments, vec![BString::from("FILE.ZIP")]);
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

#[test]
fn test_qwk_conversion_chains_sibling_replies() {
    let dir = TempDir::new().unwrap();
    let sample = qwk_mail().remove(0);
    let mail: Vec<QwkMessage> = (1..=3)
        .map(|number| {
            let mut msg = sample.clone();
            msg.msg_number = number;
            msg.ref_msg_number = if number == 1 { 0 } else { 1 };
            msg
        })
        .collect();

    let mut jam = JamMessageBase::create(dir.path().join("converted")).unwrap();
    convert_qwk_to_jam(&mail, &mut jam).unwrap();

    let parent = jam.read_header(1).unwrap();
    let first = jam.read_header(2).unwrap();
    let second = jam.read_header(3).unwrap();
    assert_eq!(parent.reply_first, 2);
    assert_eq!(first.reply_to, 1);
    assert_eq!(first.reply_next, 3);
    assert_eq!(second.reply_to, 1);
    assert_eq!(second.reply_next, 0);
    assert_eq!(first.reply_crc, second.reply_crc);
    assert_ne!(first.reply_crc, 0);
}

#[test]
fn test_reply_links_set_first_and_next() {
    use jamjam::conversion::pcboard_to_jam::wire_reply_links;
    use jamjam::jam::JamMessage;
    use std::collections::{BTreeMap, HashMap};

    let aka = EchomailAddress::default();
    let parent = JamMessage::new(&aka).with_from(BString::from("sysop"));
    let first = JamMessage::new(&aka)
        .with_from(BString::from("alice"))
        .with_reply_to(1);
    let second = JamMessage::new(&aka)
        .with_from(BString::from("bob"))
        .with_reply_to(1);
    let parent_crc = parent.msgid_crc();
    let mut messages = BTreeMap::from([(1, parent), (2, first), (3, second)]);
    let ids = HashMap::from([(1, parent_crc)]);

    wire_reply_links(&mut messages, &ids);

    assert_eq!(messages[&1].reply_first(), 2);
    assert_eq!(messages[&2].reply_next(), 3);
    assert_eq!(messages[&2].reply_to(), 1);
    assert_eq!(messages[&3].reply_to(), 1);
}
