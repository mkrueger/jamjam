use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
};

use crate::{
    jam::{
        self, JamMessage, JamMessageBase,
        msg_header::{MessageSubfield, SubfieldType},
    },
    pcboard::{PCBoardMessage, PCBoardMessageBase, message_header::ExtendedHeaderInformation},
    util::{cp437::to_utf8, echomail::EchomailAddress},
};

fn jam_attributes(msg: &PCBoardMessage) -> u32 {
    let mut result = 0;
    if matches!(
        msg.status(),
        crate::pcboard::MessageStatus::Private | crate::pcboard::MessageStatus::CommentToSysop
    ) {
        result |= jam::attributes::MSG_PRIVATE;
    }

    if msg.is_read() {
        result |= jam::attributes::MSG_READ;
    }

    if msg.is_deleted() {
        result |= jam::attributes::MSG_DELETED;
    }

    if msg.header.has_attach() {
        result |= jam::attributes::MSG_FILEATTACH;
    }

    if msg.header.has_reqrr() {
        result |= jam::attributes::MSG_RECEIPTREQ;
    }

    result
}

pub fn convert_pcboard_to_jam(
    pcboard_path: &Path,
    jam_dest_path: &Path,
    aka: &EchomailAddress,
) -> crate::Result<()> {
    let pcb_base = PCBoardMessageBase::open(pcboard_path)?;

    let mut jam_messages = BTreeMap::new();
    let mut message_ids = HashMap::new();
    // JAM numbers the messages from BaseMsgNum, which is 1 for a fresh base.
    let mut cur_msg = 1;
    let mut number_map = HashMap::new();
    for msg_result in pcb_base.iter()? {
        let msg = msg_result?;
        let attribute = jam_attributes(&msg);
        let time = msg.header.date_time();

        let mut to = msg.header.to_field;
        let mut from = msg.header.from_field;
        let mut subj = msg.header.subj_field;
        let mut sub_fields = Vec::new();
        for header in msg.extended_header {
            match header.info {
                ExtendedHeaderInformation::To => {
                    to.clone_from(&header.content);
                }
                ExtendedHeaderInformation::From => {
                    from.clone_from(&header.content);
                }
                ExtendedHeaderInformation::Subject => {
                    subj.clone_from(&header.content);
                }
                ExtendedHeaderInformation::To2 => {
                    to.append(&mut header.content.clone());
                }
                ExtendedHeaderInformation::From2 => {
                    from.append(&mut header.content.clone());
                }
                ExtendedHeaderInformation::Attach => {
                    sub_fields.push(MessageSubfield::new(SubfieldType::EnclFile, header.content));
                }

                ExtendedHeaderInformation::List
                | ExtendedHeaderInformation::Route
                | ExtendedHeaderInformation::Origin
                | ExtendedHeaderInformation::Reqrr
                | ExtendedHeaderInformation::Ackrr
                | ExtendedHeaderInformation::Ackname
                | ExtendedHeaderInformation::Packout
                | ExtendedHeaderInformation::Forward
                | ExtendedHeaderInformation::Ufollow
                | ExtendedHeaderInformation::Unewsgr => {
                    // ignore
                    continue;
                }
            }
        }

        let new_msg = JamMessage::new(aka)
            .with_reply_to(*number_map.get(&msg.header.reply_to).unwrap_or(&0))
            .with_date_time(time.and_utc())
            .with_text(to_utf8(&msg.text).into())
            .with_attributes(attribute)
            .with_password(&msg.header.password)
            .with_from(to_utf8(&from).into())
            .with_to(to_utf8(&to).into())
            .with_subject(to_utf8(&subj).into());
        if new_msg.is_deleted() {
            continue;
        }
        // Only messages that survive get a number, otherwise a reply would
        // point at whichever message reuses the slot.
        number_map.insert(msg.header.msg_number, cur_msg);
        message_ids.insert(cur_msg, new_msg.msgid_crc());
        jam_messages.insert(cur_msg, new_msg);
        cur_msg += 1;
    }

    wire_reply_links(&mut jam_messages, &message_ids);

    // write jam message base
    let mut jam_base = JamMessageBase::create(jam_dest_path)?;
    for msg in jam_messages.values() {
        jam_base.write_message(msg)?;
    }
    jam_base.write_jhr_header()?;
    Ok(())
}

/// Sets reply_crc, reply_first and reply_next from the converted numbers.
pub fn wire_reply_links(
    jam_messages: &mut BTreeMap<u32, JamMessage>,
    message_ids: &HashMap<u32, u32>,
) {
    let mut replies: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (i, msg) in jam_messages.iter_mut() {
        let reply_to = msg.reply_to();
        if reply_to != 0
            && reply_to != *i
            && let Some(crc) = message_ids.get(&reply_to)
        {
            msg.set_reply_crc(*crc);
            replies.entry(reply_to).or_default().push(*i);
        }
    }

    for (parent, children) in replies {
        let Some(first) = children.first().copied() else {
            continue;
        };
        if let Some(msg) = jam_messages.get_mut(&parent) {
            msg.set_reply_first(first);
        }
        for window in children.windows(2) {
            if let Some(msg) = jam_messages.get_mut(&window[0]) {
                msg.set_reply_next(window[1]);
            }
        }
    }
}
