use crate::{
    jam::{self, JamMessage, JamMessageBase, raw},
    qwk::qwk_message::QwkMessage,
    util::{cp437::to_utf8, echomail::EchomailAddress},
};
use std::collections::{BTreeMap, HashMap};

fn jam_attributes(msg: &QwkMessage) -> u32 {
    let mut result = 0;
    // Private, read by someone else / by the recipient / comment to sysop.
    if b"*+~`".contains(&msg.status) {
        result |= jam::attributes::MSG_PRIVATE;
    }
    if b"-+`^#".contains(&msg.status) {
        result |= jam::attributes::MSG_READ;
    }
    if msg.is_deleted() {
        result |= jam::attributes::MSG_DELETED;
    }
    result
}

/// Appends the given QWK messages to a JAM base.
///
/// The JAM base assigns the message numbers, so the reply pointers of the
/// packet are translated to the numbers the messages end up with.
pub fn convert_qwk_to_jam(
    qwk_mail: &[QwkMessage],
    jam_base: &mut JamMessageBase,
) -> crate::Result<()> {
    let aka = EchomailAddress::default();
    let mut number_map = HashMap::new();
    let mut written = Vec::with_capacity(qwk_mail.len());

    for mail in qwk_mail {
        let jam_msg = JamMessage::new(&aka)
            .with_from(to_utf8(&mail.from).into())
            .with_to(to_utf8(&mail.to).into())
            .with_subject(to_utf8(&mail.subj).into())
            .with_password(&mail.password)
            .with_date_time(mail.date_time().and_utc())
            .with_attributes(jam_attributes(mail))
            .with_text(to_utf8(&mail.text).into());

        let msgid_crc = jam_msg.msgid_crc();
        let number = jam_base.write_message(&jam_msg)?;
        number_map.insert(mail.msg_number, number);
        written.push((number, msgid_crc, mail.ref_msg_number));
    }

    link_replies(jam_base, &written, &number_map)?;
    jam_base.write_jhr_header()?;
    Ok(())
}

/// Second pass, a reply target is only known once every message has a number.
fn link_replies(
    jam_base: &mut JamMessageBase,
    written: &[(u32, u32, u32)],
    number_map: &HashMap<u32, u32>,
) -> crate::Result<()> {
    let crc_by_number: HashMap<u32, u32> = written
        .iter()
        .map(|(number, crc, _)| (*number, *crc))
        .collect();
    let mut replies: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (number, _, ref_number) in written {
        let Some(reply_to) = number_map.get(ref_number) else {
            continue;
        };
        if reply_to == number {
            continue;
        }
        replies.entry(*reply_to).or_default().push(*number);
    }

    for (reply_to, children) in &replies {
        let mut linked = Vec::with_capacity(children.len());
        for number in children {
            let Ok(mut header) = jam_base.read_header(*number) else {
                continue;
            };
            header.reply_to = *reply_to;
            if let Some(crc) = crc_by_number.get(reply_to) {
                header.reply_crc = *crc;
            }
            linked.push((*number, header));
        }

        // Chain the siblings in message order: parent -> first -> next -> ...
        let next_numbers: Vec<u32> = linked.iter().skip(1).map(|(n, _)| *n).collect();
        for (i, (number, header)) in linked.iter_mut().enumerate() {
            if let Some(next) = next_numbers.get(i) {
                header.reply_next = *next;
            }
            raw::update_header(jam_base, *number, header)?;
        }

        let Some((first, _)) = linked.first() else {
            continue;
        };
        let Ok(mut header) = jam_base.read_header(*reply_to) else {
            continue;
        };
        if header.reply_first == 0 {
            header.reply_first = *first;
            raw::update_header(jam_base, *reply_to, &header)?;
        }
    }
    Ok(())
}
