use bstr::{BString, ByteSlice};

use crate::{Error, pcboard::PCB_TXT_EOL_PTR, qwk::QwkError};
use std::io::{Read, Write};

#[derive(Clone, Debug)]
pub struct QwkMessage {
    /// Message status flags
    /// # Remarks
    /// Not Read | Read | Type
    /// ---------|------|---------------
    /// ' '      | '-'  | Public
    ///          | '*'  | Private, read by someone but not by intended recipient
    ///          | '+'  | Private, read by official recipient
    /// '~'      | '`'  | Comment to Sysop
    /// '%'      | '^'  | Sender password
    /// '!'      | '#'  | Group password
    /// '$'      |      | group password protected to all
    pub status: u8,

    /// Message number (7 bytes, saved as ascii)
    pub msg_number: u32,

    /// Date & time (MM-DD-YYHH:MM)
    pub date_time: BString,

    /// 25 character "To" field
    /// Note: May be longer because it's read from the kludge field.
    pub to: BString,

    /// 25 character "From" field
    /// Note: May be longer because it's read from the kludge field.
    pub from: BString,

    /// 25 character "Subj" field
    /// Note: May be longer because it's read from the kludge field.
    pub subj: BString,

    /// 12 character "Password" in plain text
    /// Unused - it's not making any senes at all - left over from PCBoard
    ///
    /// Note: Giving users both the message and the password to read as plain text doesn't really make sense.
    pub password: BString,

    /// Reference message number (8 bytes, saved as ascii)
    pub ref_msg_number: u32,

    /// Number of 128-bytes blocks in message incl. header, 6 char ascii
    // pub num_blocks: String, (only for documentation purposes)

    /// 225 == active, 226 == deleted
    pub active_flag: u8,

    pub conference_number: u16,

    pub logical_message_number: u16,

    /// Indicates whether the message has a network tag-line
    /// or not.  A value of '*' indicates that a network tag-
    /// line is present; a value of ' ' (space) indicates
    /// there isn't one.  Messages sent to readers (non-net-
    /// status) generally leave this as a space.  Only network
    /// softwares need this information.
    pub net_tag: u8,

    pub text: BString,
}

pub const MSG_ACTIVE: u8 = 225;
pub const MSG_INACTIVE: u8 = 226;

/// Width of the fixed "To", "From" and "Subject" header fields.
const FIELD_LEN: usize = 25;
const PASSWORD_LEN: usize = 12;
const DATE_TIME_LEN: usize = 13;

impl QwkMessage {
    pub const HEADER_SIZE: usize = 128;

    /// Returns the date and time of the message
    ///
    /// # Remarks
    /// Returns default, in case of incorrect date
    pub fn date_time(&self) -> chrono::naive::NaiveDateTime {
        chrono::NaiveDateTime::parse_from_str(&self.date_time.to_string(), "%m-%d-%y%H:%M")
            .unwrap_or_default()
    }

    pub fn is_deleted(&self) -> bool {
        self.active_flag != MSG_ACTIVE
    }

    pub fn read(mut file: impl Read, is_extended: bool) -> crate::Result<Self> {
        let data = &mut [0; Self::HEADER_SIZE];
        file.read_exact(data)?;
        let mut data = &data[..];

        convert_u8!(status, data);

        let msg_number = parse_qwk_number(&data[..7])?;
        data = &data[7..];

        convert_to_string!(date_time, data, 13);
        convert_to_string!(to_field, data, 25);
        convert_to_string!(from_field, data, 25);
        convert_to_string!(subj_field, data, 25);
        convert_to_string!(password, data, 12);

        let ref_msg_number = parse_qwk_number(&data[..8])?;
        data = &data[8..];
        let num_blocks = parse_qwk_number(&data[..6])?;
        data = &data[6..];

        convert_u8!(active_flag, data);
        convert_u16!(conference_number, data);
        convert_u16!(logical_message_number, data);
        convert_u8!(net_tag, data);

        if num_blocks == 0 {
            return Err(Error::qwk(
                0,
                "message header declares zero 128 byte blocks",
            ));
        }
        let text_len = (num_blocks as usize - 1) * Self::HEADER_SIZE;
        let mut text = Vec::new();
        file.by_ref().take(text_len as u64).read_to_end(&mut text)?;
        if text.len() != text_len {
            return Err(Error::qwk(
                0,
                format!(
                    "message text truncated, expected {text_len} bytes, got {}",
                    text.len()
                ),
            ));
        }
        let mut text: BString = crate::pcboard::convert_msg(&text);
        let mut to_field = to_field;
        let mut from_field = from_field;
        let mut subj_field = subj_field;

        if is_extended {
            loop {
                let kludge = get_kludge(&text);
                if kludge == 0 {
                    break;
                }
                let Some(line) = text.lines().next() else {
                    break;
                };
                let consumed = (line.len() + 1).min(text.len());
                match kludge {
                    1 => to_field = line[4..].into(),   // "To: "
                    2 => from_field = line[6..].into(), // "From: "
                    3 => subj_field = line[9..].into(), // "Subject: "
                    _ => {}
                }
                text = text[consumed..].into();
            }
        }

        Ok(Self {
            status,
            msg_number,
            date_time,
            to: to_field,
            from: from_field,
            subj: subj_field,
            password,
            ref_msg_number,
            active_flag,
            conference_number,
            logical_message_number,
            net_tag,
            text,
        })
    }

    /// Writes the 128 byte header followed by the message blocks.
    ///
    /// Fields wider than their fixed slot are truncated, the full value travels
    /// in the kludge lines of the data block when `is_extended` is set.
    pub fn write(&self, mut file: impl Write, is_extended: bool) -> crate::Result<usize> {
        let data = self.generate_data_block(is_extended);
        let num_blocks = data.len() / Self::HEADER_SIZE + 1;

        let mut header: Vec<u8> = Vec::with_capacity(Self::HEADER_SIZE);
        header.push(self.status);
        header.extend(ascii_number(self.msg_number, 7)?);
        header.extend(fixed_field(&self.date_time, DATE_TIME_LEN));
        header.extend(fixed_field(&self.to, FIELD_LEN));
        header.extend(fixed_field(&self.from, FIELD_LEN));
        header.extend(fixed_field(&self.subj, FIELD_LEN));
        header.extend(fixed_field(&self.password, PASSWORD_LEN));
        header.extend(ascii_number(self.ref_msg_number, 8)?);
        header.extend(ascii_number(num_blocks as u32, 6)?);
        header.push(self.active_flag);
        header.extend(&self.conference_number.to_le_bytes());
        header.extend(&self.logical_message_number.to_le_bytes());
        header.push(self.net_tag);

        debug_assert_eq!(header.len(), Self::HEADER_SIZE);
        file.write_all(&header)?;
        file.write_all(&data)?;
        Ok(num_blocks)
    }

    fn generate_data_block(&self, is_extended: bool) -> Vec<u8> {
        let mut res = Vec::new();

        if is_extended {
            if self.to.len() > FIELD_LEN {
                res.extend(b"To: ");
                res.extend_from_slice(&self.to);
                res.extend(PCB_TXT_EOL_PTR);
            }
            if self.from.len() > FIELD_LEN {
                res.extend(b"From: ");
                res.extend_from_slice(&self.from);
                res.extend(PCB_TXT_EOL_PTR);
            }

            if self.subj.len() > FIELD_LEN {
                res.extend(b"Subject: ");
                res.extend_from_slice(&self.subj);
                res.extend(PCB_TXT_EOL_PTR);
            }
            // According to the spec after the kludge a blank line should be put.
            if self.to.len() > FIELD_LEN
                || self.from.len() > FIELD_LEN
                || self.subj.len() > FIELD_LEN
            {
                res.extend(PCB_TXT_EOL_PTR);
            }
        }
        for b in self.text.iter() {
            if *b == b'\n' {
                res.extend(PCB_TXT_EOL_PTR);
            } else {
                res.push(*b);
            }
        }
        if res.len() % 128 != 0 {
            res.resize(res.len() + 128 - res.len() % 128, b' ');
        }
        res
    }
}

fn parse_qwk_number(data: &[u8]) -> crate::Result<u32> {
    let mut number: u32 = 0;
    for &b in data {
        if b == b' ' || b == 0 {
            break;
        }
        if !b.is_ascii_digit() {
            return Err(QwkError::InvalidMessageNumber.into());
        }
        number = number
            .checked_mul(10)
            .and_then(|n| n.checked_add((b - b'0') as u32))
            .ok_or(QwkError::InvalidMessageNumber)?;
    }

    Ok(number)
}

/// Truncates or space pads a value to the width its header slot has.
fn fixed_field(field: &BString, len: usize) -> Vec<u8> {
    let mut data = field.to_vec();
    data.truncate(len);
    data.resize(len, b' ');
    data
}

fn ascii_number(value: u32, len: usize) -> crate::Result<Vec<u8>> {
    let mut data = value.to_string().into_bytes();
    if data.len() > len {
        return Err(Error::qwk(
            0,
            format!("{value} does not fit into {len} ascii digits"),
        ));
    }
    data.resize(len, b' ');
    Ok(data)
}

fn get_kludge(text: &BString) -> u8 {
    if text.starts_with(b"To: ") {
        return 1;
    }
    if text.starts_with(b"From: ") {
        return 2;
    }
    if text.starts_with(b"Subject: ") {
        return 3;
    }
    0
}
