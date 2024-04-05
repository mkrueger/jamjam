use std::{
    fs::{self, File},
    io::{BufReader, Read, Seek},
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{convert_u32, util::basic_real::basicreal_to_u32};

use self::{
    base_header::MessageBaseHeader,
    message_header::{ExtendedHeader, MessageHeader},
    message_index::MessageIndex,
};

mod base_header;
mod message_header;
mod message_index;

#[cfg(test)]
mod tests;

/*
const FROM_TO_LEN: usize = 25;
const PASSWORD_LEN: usize = 12;
const DATE_LEN: usize = 8;
const TIME_LEN: usize = 5;*/

#[derive(Error, Debug)]
pub enum PCBoardError {
    #[error("Message number {0} out of range. Valid range is {1}..={2}")]
    MessageNumberOutOfRange(u32, u32, u32),
}

mod extensions {
    /// filename.JHR - Message header data
    pub const INDEX: &str = "idx";

    /// filename.NDX - Old message index (optional)
    pub const OLD_INDEX: &str = "ndx";
}

fn convert_block(buf: &[u8]) -> String {
    let mut str = String::new();
    for c in buf {
        if *c == 0 {
            break;
        }
        // str.push(CP437_TO_UNICODE[*c as usize]);
        str.push(*c as char);
    }
    str
}
fn convert_str(buf: &[u8]) -> String {
    let mut str = convert_block(buf);
    while str.ends_with([' ']) {
        str.pop();
    }
    str
}

fn _gen_string(str: &str, num: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    for c in str.chars().take(num) {
        buf.push(c as u8);
    }
    while buf.len() < num {
        buf.push(b' ');
    }
    buf
}

pub struct Message {
    pub header: MessageHeader,
    pub extended_header: Vec<ExtendedHeader>,
    pub text: String,
}

impl Message {
    pub fn read(file: &mut BufReader<File>) -> crate::Result<Self> {
        let mut text = String::new();

        let header = MessageHeader::read(file)?;

        let mut buf = vec![0; 128 * ((header.num_blocks as usize).saturating_sub(1))];
        file.read_exact(&mut buf)?;
        let mut i = 0;

        let mut extended_header = Vec::new();
        while i < buf.len() {
            if buf[i] == 0xFF && buf[i + 1] == 0x40 {
                extended_header.push(ExtendedHeader::deserialize(&buf[i..])?);
                i += 0x48;
                continue;
            }
            text = Self::convert_msg(&buf[i..]);
            break;
        }

        Ok(Message {
            header,
            extended_header,
            text,
        })
    }

    fn convert_msg(buf: &[u8]) -> String {
        let mut str = String::new();
        for c in buf {
            if *c == 0 {
                continue;
            }
            if *c == 0x0D || *c == 0xE3 {
                str.push('\n');
            } else {
                str.push(*c as char);
            }
        }
        str
    }
}

pub struct MessageBase {
    file_name: PathBuf,
    header_info: MessageBaseHeader,
}

impl MessageBase {
    /// opens an existing message base with base path (without any extension)
    pub fn open<P: AsRef<Path>>(file_name: P) -> crate::Result<Self> {
        let header_info = MessageBaseHeader::load(&mut File::open(&file_name)?)?;
        Ok(Self {
            file_name: file_name.as_ref().into(),
            header_info,
        })
    }

    /// Number of active (not deleted) msgs  
    pub fn active_messages(&self) -> u32 {
        self.header_info.active_msgs
    }

    pub fn highest_message_number(&self) -> u32 {
        self.header_info.high_msg_num
    }

    pub fn lowest_message_number(&self) -> u32 {
        self.header_info.low_msg_num
    }

    pub fn callers(&self) -> u32 {
        self.header_info.callers
    }

    pub fn read_message(&self, num: u32) -> crate::Result<Message> {
        if num < self.lowest_message_number() || num > self.highest_message_number() {
            return Err(PCBoardError::MessageNumberOutOfRange(
                num,
                self.lowest_message_number(),
                self.highest_message_number(),
            )
            .into());
        }
        let idx_file_name = self.file_name.with_extension(extensions::INDEX);
        let mut reader = BufReader::new(File::open(idx_file_name)?);
        reader.seek(std::io::SeekFrom::Start(
            (num as u64 - 1) * MessageIndex::HEADER_SIZE as u64,
        ))?;
        let header = MessageIndex::read(&mut reader)?;
        let mut file = BufReader::new(File::open(&self.file_name)?);
        file.seek(std::io::SeekFrom::Start(header.offset as u64))?;
        Message::read(&mut file)
    }

    pub fn read_old_index(&self) -> crate::Result<Vec<u32>> {
        let old_idx_file_name = self.file_name.with_extension(extensions::OLD_INDEX);

        let mut res = Vec::new();
        let bytes = fs::read(old_idx_file_name)?;

        let mut data = &bytes[..];
        while data.len() >= 4 {
            convert_u32!(num, data);
            if num == 0 {
                break;
            }
            let num = (basicreal_to_u32(num) - 1) * 128;
            res.push(num);
        }

        Ok(res)
    }

    pub fn read_index(&self) -> crate::Result<Vec<MessageIndex>> {
        let idx_file_name = self.file_name.with_extension(extensions::INDEX);

        let mut res = Vec::new();
        let mut reader = BufReader::new(File::open(idx_file_name)?);

        while let Ok(header) = MessageIndex::read(&mut reader) {
            res.push(header);
        }

        Ok(res)
    }
}
