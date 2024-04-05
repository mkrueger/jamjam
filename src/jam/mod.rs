use std::fs::{self, OpenOptions};
use std::io::{BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fs::File, io::Read};

use rayon::iter::{IntoParallelIterator, ParallelIterator};
use thiserror::Error;

use crate::util::crc32::{self, CRC_SEED};
use crate::{convert_single_u32, convert_u32};

use self::jhr_header::JHRHeaderInfo;
use self::last_read_storage::LastReadStorage;
use self::msg_header::MessageHeader;

pub mod jhr_header;
pub mod last_read_storage;
pub mod msg_header;

#[cfg(test)]
mod tests;

#[derive(Error, Debug)]
pub enum JamError {
    #[error("Invalid header signature (needs to start with 'JAM\\0')")]
    InvalidHeaderSignature,

    #[error("Index file corrupted")]
    IndexFileCorrupted,

    #[error("Unsupported message header revision: {0}")]
    UnsupportedMessageHeaderRevision(u16),

    #[error("Invalid subfield length {0} for sub field {1}")]
    InvalidSubfieldLength(u32, usize),

    #[error("Message number {0} out of range. Valid range is {1}..={2}")]
    MessageNumberOutOfRange(u32, u32, u32),
}

mod extensions {
    /// filename.JHR - Message header data
    pub const HEADER_DATA: &str = "jhr";

    /// filename.JDT - Message text data
    pub const TEXT_DATA: &str = "jdt";

    /// filename.JDX - Message index
    pub const MESSAGE_INDEX: &str = "jdx";

    /// filename.JLR - Lastread information
    pub const LASTREAD_INFO: &str = "jlr";
}

const JAM_SIGNATURE: [u8; 4] = [b'J', b'A', b'M', 0];

pub struct MessageBase {
    file_name: PathBuf,
    header_info: JHRHeaderInfo,
    last_read_record: i32,
    is_locked: bool,
}

impl MessageBase {
    /// opens an existing message base with base path (without any extension)
    pub fn open<P: AsRef<Path>>(file_name: P) -> crate::Result<Self> {
        let header_file_name = file_name.as_ref().with_extension(extensions::HEADER_DATA);
        let header_info = JHRHeaderInfo::load(&mut File::open(header_file_name)?)?;
        Ok(Self {
            file_name: file_name.as_ref().into(),
            header_info,
            last_read_record: -1,
            is_locked: false,
        })
    }

    pub fn get_info(&self) -> &JHRHeaderInfo {
        &self.header_info
    }

    /// Update counter
    pub fn mod_counter(&self) -> u32 {
        self.header_info.mod_counter
    }

    /// Lowest message number in index file
    ///
    /// # Remarks
    /// This field determines the lowest message number in the index file.
    /// The value for this field is one (1) when a message area is first
    /// created. By using this field, a message area can be packed (deleted
    /// messages are removed) without renumbering it. If BaseMsgNum contains
    /// 500, the first index record points to message number 500.
    ///
    /// BaseMsgNum has to be taken into account when an application
    /// calculates the next available message number (for creating new
    /// messages) as well as the highest and lowest message number in a
    /// message area.
    pub fn base_messagenumber(&self) -> u32 {
        self.header_info.base_msg_num
    }

    /// Number of active (not deleted) msgs  
    pub fn active_messages(&self) -> u32 {
        self.header_info.active_msgs
    }

    /// True, if a password is required to access this msg base
    pub fn needs_password(&self) -> bool {
        self.header_info.password_crc != CRC_SEED
    }

    /// Checks if a password is valid.
    pub fn is_password_valid(&self, password: &str) -> bool {
        self.header_info.password_crc == CRC_SEED
            || self.header_info.password_crc == Self::get_crc(password)
    }

    pub fn create<P: AsRef<Path>>(file_name: P) -> crate::Result<Self> {
        Self::create_with_passwordcrc(file_name, CRC_SEED)
    }

    pub fn create_with_password<P: AsRef<Path>>(
        file_name: P,
        password: &str,
    ) -> crate::Result<Self> {
        Self::create_with_passwordcrc(file_name, Self::get_crc(password))
    }

    pub fn create_with_passwordcrc<P: AsRef<Path>>(
        file_name: P,
        passwordcrc: u32,
    ) -> crate::Result<Self> {
        let header_file_name = file_name.as_ref().with_extension(extensions::HEADER_DATA);
        JHRHeaderInfo::create(&header_file_name, passwordcrc)?;
        fs::write(file_name.as_ref().with_extension(extensions::TEXT_DATA), "")?;
        fs::write(
            file_name.as_ref().with_extension(extensions::MESSAGE_INDEX),
            "",
        )?;
        fs::write(
            file_name.as_ref().with_extension(extensions::LASTREAD_INFO),
            "",
        )?;
        Self::open(file_name)
    }

    pub fn delete_message_base(&self) -> crate::Result<()> {
        fs::remove_file(self.file_name.with_extension(extensions::HEADER_DATA))?;
        fs::remove_file(self.file_name.with_extension(extensions::TEXT_DATA))?;
        fs::remove_file(self.file_name.with_extension(extensions::MESSAGE_INDEX))?;
        fs::remove_file(self.file_name.with_extension(extensions::LASTREAD_INFO))?;
        Ok(())
    }

    pub fn lock(&mut self) -> crate::Result<bool> {
        if self.is_locked {
            return Ok(false);
        }
        self.is_locked = true;
        Ok(true)
    }

    pub fn unlock(&mut self) -> crate::Result<()> {
        self.is_locked = false;
        Ok(())
    }

    /// Get the jam base crc of a string
    ///
    /// This is the lowercase z-modem crc32
    pub fn get_crc(str: &str) -> u32 {
        crc32::get_crc32(str.to_ascii_lowercase().as_bytes())
    }

    /// Writes a message header + text directly
    /// Note that this is a dangerous operation, as it does not update the jhr header.
    /// `write_jhr_header` needs to be called after all messages are written.
    pub fn write_message(&mut self, mut header: MessageHeader, text: &str) -> crate::Result<()> {
        let text_file_name = self.file_name.with_extension(extensions::TEXT_DATA);
        let mut text_file = OpenOptions::new().append(true).open(text_file_name)?;
        header.message_number = self.header_info.base_msg_num + self.header_info.active_msgs;
        let now = SystemTime::now();
        let unix_time = now.duration_since(UNIX_EPOCH)?;
        header.date_written = unix_time.as_secs() as u32;
        header.offset = text_file.metadata()?.len() as u32;
        header.txt_len = text.len() as u32;
        text_file.write_all(text.as_bytes())?;

        let header_path = self.file_name.with_extension(extensions::HEADER_DATA);
        let header_file = OpenOptions::new().append(true).open(header_path)?;
        let message_header_offset = header_file.metadata()?.len() as u32;

        let mut writer = BufWriter::new(header_file);
        header.write(&mut writer)?;
        writer.flush()?;
        self.header_info.active_msgs += 1;

        let index_file_name = self.file_name.with_extension(extensions::MESSAGE_INDEX);
        let mut index_file = OpenOptions::new().append(true).open(index_file_name)?;

        let crc = if let Some(to) = header.get_to() {
            Self::get_crc(&to)
        } else {
            CRC_SEED
        };
        index_file.write_all(&crc.to_le_bytes())?;
        index_file.write_all(&message_header_offset.to_le_bytes())?;

        Ok(())
    }

    /// Writes the current header to disk.
    pub fn write_jhr_header(&mut self) -> crate::Result<()> {
        let header_path = self.file_name.with_extension(extensions::HEADER_DATA);
        let header_file = OpenOptions::new().write(true).open(header_path)?;
        let mut writer = BufWriter::new(header_file);
        self.header_info.update(&mut writer)?;
        writer.flush()?;
        Ok(())
    }

    /// Updates header with the one from disk.
    /// Usually it's not required to call that (only for outside changes detected)
    pub fn read_jhr_header(&mut self) -> crate::Result<()> {
        let header_file_name = self.file_name.with_extension(extensions::HEADER_DATA);
        let mut header = File::open(header_file_name)?;
        self.header_info = JHRHeaderInfo::load(&mut header)?;
        Ok(())
    }

    pub fn get_msg_text(&self, header: &MessageHeader) -> crate::Result<String> {
        let text_file_name = self.file_name.with_extension(extensions::TEXT_DATA);
        let mut text_file = File::open(text_file_name)?;
        text_file.seek(SeekFrom::Start(header.offset as u64))?;
        let mut buffer = vec![0; header.txt_len as usize];
        text_file.read_exact(&mut buffer)?;

        let mut res = String::new();

        for b in buffer {
            res.push(b as char);
            if b == b'\r' {
                res.push('\n');
            }
        }

        Ok(res)
    }

    pub fn read_headers(&self) -> crate::Result<Vec<MessageHeader>> {
        let header_file_name = self.file_name.with_extension(extensions::HEADER_DATA);
        let header_file = File::open(header_file_name)?;
        let mut reader = BufReader::new(header_file);
        reader.seek(SeekFrom::Start(JHRHeaderInfo::JHR_HEADER_SIZE))?;
        let mut res = Vec::new();
        while let Ok(header) = MessageHeader::read(&mut reader) {
            res.push(header);
        }
        Ok(res)
    }

    pub fn get_header(&self, header_number: u32) -> crate::Result<MessageHeader> {
        let header_file_name = self.file_name.with_extension(extensions::HEADER_DATA);
        if header_number < self.header_info.base_msg_num
            || header_number > self.header_info.active_msgs
        {
            return Err(JamError::MessageNumberOutOfRange(
                header_number,
                self.header_info.base_msg_num,
                self.header_info.active_msgs,
            )
            .into());
        }
        let record = (header_number - self.header_info.base_msg_num) as u64;

        let index_file_name = self.file_name.with_extension(extensions::MESSAGE_INDEX);
        let mut index_file = OpenOptions::new().read(true).open(index_file_name)?;
        index_file.seek(SeekFrom::Start(record * 8 + 4))?;
        let mut offset = [0; 4];
        index_file.read_exact(&mut offset)?;
        let offset = u32::from_le_bytes(offset);
        let mut header_file = File::open(header_file_name)?;
        header_file.seek(SeekFrom::Start(offset as u64))?;
        let mut reader = BufReader::new(header_file);
        MessageHeader::read(&mut reader)
    }

    pub fn read_last_read_file(&self) -> crate::Result<Vec<LastReadStorage>> {
        let last_read_file_name = self.file_name.with_extension(extensions::LASTREAD_INFO);
        let last_read_file = File::open(last_read_file_name)?;
        let mut res = Vec::new();
        let mut reader = BufReader::new(last_read_file);
        while let Ok(last_read) = LastReadStorage::load(&mut reader) {
            res.push(last_read);
        }
        Ok(res)
    }

    pub fn find_last_read(
        &mut self,
        user_name_crc: u32,
        id: u32,
    ) -> crate::Result<Option<LastReadStorage>> {
        let last_read_file_name = self.file_name.with_extension(extensions::LASTREAD_INFO);
        let file = File::open(last_read_file_name)?;
        let mut reader = BufReader::new(file);

        let id_bytes = id.to_le_bytes();
        let crc_bytes = user_name_crc.to_le_bytes();

        let needle = [
            crc_bytes[0],
            crc_bytes[1],
            crc_bytes[2],
            crc_bytes[3],
            id_bytes[0],
            id_bytes[1],
            id_bytes[2],
            id_bytes[3],
        ];
        let data = &mut [0; 16];
        let mut record_number = 0;
        while reader.read_exact(data).is_ok() {
            if data.starts_with(&needle) {
                self.last_read_record = record_number;
                let mut data_c = &data[8..];
                convert_u32!(last_read_msg, data_c);
                convert_u32!(high_read_msg, data_c);
                return Ok(Some(LastReadStorage {
                    user_crc: user_name_crc,
                    user_id: id,
                    last_read_msg,
                    high_read_msg,
                }));
            }
            record_number += 1;
        }
        Ok(None)
    }

    /// Gixes back all the record number (+BaseMsgNum) within the .JDX file determines a message's number for a given user.
    pub fn search_message_index(&self, crc: u32) -> crate::Result<Vec<u32>> {
        let index_file_name = self.file_name.with_extension(extensions::MESSAGE_INDEX);
        let index_file = fs::read(index_file_name)?;

        if index_file.len() % 8 != 0 {
            return Err(Box::new(JamError::IndexFileCorrupted));
        }
        // all indices need to be scanned so it can be done in parallel
        let needle = crc.to_le_bytes();
        let res = (0..index_file.len() / 8)
            .into_par_iter()
            .filter(|o| {
                let i = o << 3;
                index_file[i..].starts_with(&needle)
            })
            .map(|i| {
                let data = &index_file[i + 4..];
                convert_single_u32!(msg_num, data);
                msg_num
            })
            .collect();
        Ok(res)
    }

    /* Single threaded version:
    pub fn search_index(&self, crc: u32) -> Result<Vec<u32>, Box<dyn Error>> {
        let index_file_name = self.file_name.with_extension(extensions::MESSAGE_INDEX);

        let mut res = Vec::new();

        let mut index_file = fs::read(index_file_name)?;
        let needle = crc.to_le_bytes();

        let mut i = 0;
        while i < index_file.len() {
            if index_file[i..].starts_with(&needle) {
                let mut data = &mut index_file[i + 4..];
                convert_u32!(msg_num, data);
                res.push(msg_num);
            }
            i += 8;
        }
        Ok(res)
    }*/
}

pub mod attributes {
    /// Msg created locally
    pub const MSG_LOCAL: u32 = 0x00000001;
    /// Msg is in-transit
    pub const MSG_INTRANSIT: u32 = 0x00000002;
    /// Private
    pub const MSG_PRIVATE: u32 = 0x00000004;
    /// Read by addressee
    pub const MSG_READ: u32 = 0x00000008;
    /// Sent to remote
    pub const MSG_SENT: u32 = 0x00000010;
    /// Kill when sent
    pub const MSG_KILLSENT: u32 = 0x00000020;
    /// Archive when sent
    pub const MSG_ARCHIVESENT: u32 = 0x00000040;
    /// Hold for pick-up
    pub const MSG_HOLD: u32 = 0x00000080;
    /// Crash
    pub const MSG_CRASH: u32 = 0x00000100;
    /// Send Msg now, ignore restrictions
    pub const MSG_IMMEDIATE: u32 = 0x00000200;
    /// Send directly to destination
    pub const MSG_DIRECT: u32 = 0x00000400;
    /// Send via gateway
    pub const MSG_GATE: u32 = 0x00000800;
    /// File request
    pub const MSG_FILEREQUEST: u32 = 0x00001000;
    /// File(s) attached to Msg
    pub const MSG_FILEATTACH: u32 = 0x00002000;
    /// Truncate file(s) when sent
    pub const MSG_TRUNCFILE: u32 = 0x00004000;
    /// Delete file(s) when sent
    pub const MSG_KILLFILE: u32 = 0x00008000;
    /// Return receipt requested
    pub const MSG_RECEIPTREQ: u32 = 0x00010000;
    /// Confirmation receipt requested
    pub const MSG_CONFIRMREQ: u32 = 0x00020000;
    /// Unknown destination
    pub const MSG_ORPHAN: u32 = 0x00040000;
    /// Msg text is encrypted
    ///
    /// This revision of JAM does not include compression, encryption, or
    /// escaping. The bits are reserved for future use.
    pub const MSG_ENCRYPT: u32 = 0x00080000;
    /// Msg text is compressed
    ///
    /// This revision of JAM does not include compression, encryption, or
    /// escaping. The bits are reserved for future use.
    pub const MSG_COMPRESS: u32 = 0x00100000;
    /// Msg text is seven bit ASCII
    ///
    /// This revision of JAM does not include compression, encryption, or
    /// escaping. The bits are reserved for future use.
    pub const MSG_ESCAPED: u32 = 0x00200000;
    /// Force pickup
    pub const MSG_FPU: u32 = 0x00400000;
    /// Msg is for local use only
    pub const MSG_TYPELOCAL: u32 = 0x00800000;
    /// Msg is for conference distribution
    pub const MSG_TYPEECHO: u32 = 0x01000000;
    /// Msg is direct network mail
    pub const MSG_TYPENET: u32 = 0x02000000;
    /// Msg may not be displayed to user
    pub const MSG_NODISP: u32 = 0x20000000;
    /// Msg is locked, no editing possible
    pub const MSG_LOCKED: u32 = 0x40000000;
    /// Msg is deleted
    pub const MSG_DELETED: u32 = 0x80000000;
}
