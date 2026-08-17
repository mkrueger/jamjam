use std::fs::{self, OpenOptions};
use std::io::{BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fs::File, io::Read};

use bstr::BString;
use chrono::{DateTime, Utc};
use rayon::iter::{IndexedParallelIterator, ParallelIterator};
use rayon::slice::ParallelSlice;
use thiserror::Error;

use crate::util::crc32::{self, CRC_SEED};
use crate::util::echomail::EchomailAddress;

use self::jhr_header::JhrHeaderInfo;
use self::last_read_storage::JamLastReadStorage;
use self::msg_header::{JamMessageHeader, MessageSubfield, SubfieldType};
use self::pack::EMPTY_SLOT;

pub mod jhr_header;
pub mod last_read_storage;
pub mod msg_header;
pub mod pack;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod pack_tests;

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

    #[error("Message was deleted")]
    MessageDeleted,

    #[error("Index file corrupt at record {0} (file length: {1})")]
    IndexFileCorrupt(u64, u64),
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

/// Two ulongs: CRC-32 of the recipient and the header offset in the .JHR file.
pub(crate) const INDEX_RECORD_SIZE: usize = 8;

/// Below this many index records a plain scan beats spawning rayon tasks.
const PARALLEL_SEARCH_THRESHOLD: usize = 8192;

pub struct JamMessageBase {
    file_name: PathBuf,
    header_info: JhrHeaderInfo,
    /// Number of records in the .JDX file, including the slots of deleted messages.
    index_records: u32,
    last_read_record: i32,
    locked: AtomicBool,
}

impl JamMessageBase {
    /// opens an existing message base with base path (without any extension)
    pub fn open<P: AsRef<Path>>(file_name: P) -> crate::Result<Self> {
        let file_name = file_name.as_ref();
        let header_file_name = file_name.with_extension(extensions::HEADER_DATA);
        let mut file = File::open(&header_file_name).inspect_err(|err| {
            log::error!("Error opening message base {}: {err}", file_name.display());
        })?;
        let header_info = JhrHeaderInfo::load(&mut file)?;
        let mut base = Self {
            file_name: file_name.into(),
            header_info,
            index_records: 0,
            last_read_record: -1,
            locked: AtomicBool::new(false),
        };
        base.index_records = base.count_index_records()?;
        Ok(base)
    }

    fn count_index_records(&self) -> crate::Result<u32> {
        let path = self.file_name.with_extension(extensions::MESSAGE_INDEX);
        let len = match fs::metadata(&path) {
            Ok(meta) => meta.len(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => 0,
            Err(err) => return Err(err.into()),
        };
        if !len.is_multiple_of(INDEX_RECORD_SIZE as u64) {
            return Err(JamError::IndexFileCorrupted.into());
        }
        Ok((len / INDEX_RECORD_SIZE as u64) as u32)
    }

    pub fn path(&self) -> &Path {
        &self.file_name
    }

    pub fn info(&self) -> &JhrHeaderInfo {
        &self.header_info
    }

    /// Update counter
    pub fn mod_counter(&self) -> u32 {
        self.header_info.mod_counter
    }

    /// Lowest message number in index file (`BaseMsgNum`).
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
    pub fn lowest_message_number(&self) -> u32 {
        self.header_info.base_msg_num
    }

    /// Highest message number the index can address.
    ///
    /// Returns `lowest_message_number() - 1` for an empty base, so that
    /// `lowest..=highest` is an empty range.
    pub fn highest_message_number(&self) -> u32 {
        self.header_info
            .base_msg_num
            .saturating_add(self.index_records)
            .saturating_sub(1)
    }

    /// Number of records in the index, deleted messages included.
    pub fn index_records(&self) -> u32 {
        self.index_records
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
    pub fn is_password_valid(&self, password: &BString) -> bool {
        self.header_info.password_crc == CRC_SEED
            || self.header_info.password_crc == Self::crc(password)
    }

    pub fn create<P: AsRef<Path>>(file_name: P) -> crate::Result<Self> {
        Self::create_with_password_crc(file_name, CRC_SEED)
    }

    pub fn create_with_password<P: AsRef<Path>>(
        file_name: P,
        password: &BString,
    ) -> crate::Result<Self> {
        Self::create_with_password_crc(file_name, Self::crc(password))
    }

    pub fn create_with_password_crc<P: AsRef<Path>>(
        file_name: P,
        passwordcrc: u32,
    ) -> crate::Result<Self> {
        let header_file_name = file_name.as_ref().with_extension(extensions::HEADER_DATA);
        if let Err(err) = JhrHeaderInfo::create(&header_file_name, passwordcrc) {
            log::error!(
                "Error creating message base: {}",
                header_file_name.display()
            );
            return Err(err);
        }
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

    /// Locks the message base.
    /// User is responsible for locking/unlocking.
    ///
    /// Note that locking is just process only.
    pub fn lock(&self) {
        while self.locked.swap(true, std::sync::atomic::Ordering::Acquire) {
            std::hint::spin_loop();
        }
    }

    /// Unlocks the message base
    pub fn unlock(&self) {
        self.locked
            .store(false, std::sync::atomic::Ordering::Release);
    }

    /// Locks the message base until the returned guard is dropped.
    pub fn lock_guard(&self) -> JamLockGuard<'_> {
        self.lock();
        JamLockGuard { base: self }
    }

    /// Get the jam base crc of a string
    ///
    /// This is the lowercase z-modem crc32
    pub fn crc(str: &BString) -> u32 {
        let mut str = str.clone();
        str.make_ascii_lowercase();
        let crc = crc32::checksum(&str);
        crc ^ CRC_SEED
    }

    /// Appends a message and updates the index and the base header.
    ///
    /// The message number is derived from the index, so a base that was packed
    /// or renumbered continues where its index left off. On failure the three
    /// files are truncated back to the length they had before.
    pub fn write_message(&mut self, message: &JamMessage) -> crate::Result<u32> {
        let text_path = self.file_name.with_extension(extensions::TEXT_DATA);
        let header_path = self.file_name.with_extension(extensions::HEADER_DATA);
        let index_path = self.file_name.with_extension(extensions::MESSAGE_INDEX);

        let text_len = file_len(&text_path)?;
        let header_len = file_len(&header_path)?;
        let index_len = file_len(&index_path)?;

        let msg_number = self.highest_message_number().saturating_add(1);
        match Self::append_message(
            message,
            msg_number,
            &text_path,
            &header_path,
            &index_path,
            text_len,
            header_len,
        ) {
            Ok(()) => {
                self.index_records += 1;
                if !message.is_deleted() {
                    self.header_info.active_msgs += 1;
                }
                self.write_jhr_header()?;
                Ok(msg_number)
            }
            Err(err) => {
                rollback(&text_path, text_len);
                rollback(&header_path, header_len);
                rollback(&index_path, index_len);
                Err(err)
            }
        }
    }

    fn append_message(
        message: &JamMessage,
        msg_number: u32,
        text_path: &Path,
        header_path: &Path,
        index_path: &Path,
        text_offset: u64,
        header_offset: u64,
    ) -> crate::Result<()> {
        let mut header = message.create_jam_header();
        header.message_number = msg_number;
        header.offset = text_offset as u32;
        header.txt_len = message.text().len() as u32;

        let mut text_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(text_path)?;
        text_file.write_all(message.text())?;
        text_file.flush()?;

        let header_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(header_path)?;
        let mut writer = BufWriter::new(header_file);
        header.write(&mut writer)?;
        writer.flush()?;

        let mut index_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(index_path)?;
        let crc = header.to().map_or(CRC_SEED, Self::crc);
        index_file.write_all(&crc.to_le_bytes())?;
        index_file.write_all(&(header_offset as u32).to_le_bytes())?;
        index_file.flush()?;
        Ok(())
    }

    /// Writes the current header to disk.
    pub fn write_jhr_header(&mut self) -> crate::Result<()> {
        let header_path = self.file_name.with_extension(extensions::HEADER_DATA);
        let header_file = OpenOptions::new()
            .write(true)
            .truncate(false)
            .open(header_path)?;
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
        self.header_info = JhrHeaderInfo::load(&mut header)?;
        self.index_records = self.count_index_records()?;
        Ok(())
    }

    pub fn read_message_text(&self, header: &JamMessageHeader) -> crate::Result<BString> {
        let text_file_name = self.file_name.with_extension(extensions::TEXT_DATA);
        let mut text_file = File::open(text_file_name)?;
        text_file.seek(SeekFrom::Start(header.offset as u64))?;
        let mut buffer = vec![0; header.txt_len as usize];
        text_file.read_exact(&mut buffer)?;
        Ok(BString::new(buffer))
    }

    pub fn read_headers(&self) -> crate::Result<Vec<JamMessageHeader>> {
        let header_file_name = self.file_name.with_extension(extensions::HEADER_DATA);
        let header_file = File::open(header_file_name)?;
        let mut reader = BufReader::new(header_file);
        reader.seek(SeekFrom::Start(JhrHeaderInfo::JHR_HEADER_SIZE))?;
        let mut res = Vec::new();
        while let Ok(header) = JamMessageHeader::read(&mut reader) {
            res.push(header);
        }
        Ok(res)
    }

    pub fn read_header(&self, msg_number: u32) -> crate::Result<JamMessageHeader> {
        let offset = self.header_offset(msg_number)?;

        let header_file_name = self.file_name.with_extension(extensions::HEADER_DATA);
        let mut header_file = File::open(header_file_name)?;
        header_file.seek(SeekFrom::Start(offset))?;
        let mut reader = BufReader::new(header_file);
        let header = JamMessageHeader::read(&mut reader)?;

        if header.is_deleted() {
            return Err(JamError::MessageDeleted.into());
        }
        Ok(header)
    }

    /// Index record a message number maps to.
    fn index_record(&self, msg_number: u32) -> crate::Result<u64> {
        let low = self.lowest_message_number();
        let high = self.highest_message_number();
        if self.index_records == 0 || msg_number < low || msg_number > high {
            return Err(JamError::MessageNumberOutOfRange(msg_number, low, high).into());
        }
        Ok((msg_number - low) as u64)
    }

    /// Byte offset of a message header inside the .JHR file.
    fn header_offset(&self, msg_number: u32) -> crate::Result<u64> {
        let record = self.index_record(msg_number)?;
        let index_file_name = self.file_name.with_extension(extensions::MESSAGE_INDEX);
        let mut index_file = File::open(index_file_name)?;
        let len = index_file.metadata()?.len();
        if index_file
            .seek(SeekFrom::Start(record * INDEX_RECORD_SIZE as u64 + 4))
            .is_err()
        {
            return Err(JamError::IndexFileCorrupt(record, len).into());
        }
        let mut offset = [0; 4];
        if let Err(err) = index_file.read_exact(&mut offset) {
            log::error!("Error reading index file: {err}");
            return Err(JamError::IndexFileCorrupt(record, len).into());
        }
        let offset = u32::from_le_bytes(offset);
        if offset == EMPTY_SLOT {
            return Err(JamError::MessageDeleted.into());
        }
        Ok(offset as u64)
    }

    /// Turns attribute bits on and off without touching the rest of the header.
    /// Unlike `read_header` this also reaches deleted messages.
    ///
    /// Returns whether the attributes actually changed.
    pub fn set_attributes(&self, msg_number: u32, set: u32, clear: u32) -> crate::Result<bool> {
        let offset = self.header_offset(msg_number)?;
        let header_file_name = self.file_name.with_extension(extensions::HEADER_DATA);
        let mut header_file = File::open(&header_file_name)?;
        header_file.seek(SeekFrom::Start(offset))?;
        let mut reader = BufReader::new(header_file);
        let mut header = JamMessageHeader::read(&mut reader)?;

        let attributes = (header.attributes | set) & !clear;
        if attributes == header.attributes {
            return Ok(false);
        }
        header.attributes = attributes;

        let mut header_file = OpenOptions::new()
            .write(true)
            .truncate(false)
            .open(header_file_name)?;
        header_file.seek(SeekFrom::Start(offset))?;
        let mut writer = BufWriter::new(header_file);
        header.write(&mut writer)?;
        writer.flush()?;
        Ok(true)
    }

    /// Replaces a message header. The subfields can change length, so the new
    /// record goes to the end of the header file and the index is pointed at it.
    ///
    /// As the format requires, the old record is marked deleted and its text
    /// length is zeroed so that packing does not reclaim the text that now
    /// belongs to the new record.
    pub fn update_header(&self, msg_number: u32, header: &JamMessageHeader) -> crate::Result<()> {
        let old_offset = self.header_offset(msg_number)?;
        let record = self.index_record(msg_number)?;

        let header_path = self.file_name.with_extension(extensions::HEADER_DATA);
        let header_file = OpenOptions::new().append(true).open(&header_path)?;
        let offset = header_file.metadata()?.len() as u32;
        let mut writer = BufWriter::new(header_file);
        header.write(&mut writer)?;
        writer.flush()?;

        let index_file_name = self.file_name.with_extension(extensions::MESSAGE_INDEX);
        let mut index_file = OpenOptions::new()
            .write(true)
            .truncate(false)
            .open(index_file_name)?;
        index_file.seek(SeekFrom::Start(record * INDEX_RECORD_SIZE as u64))?;
        let crc = header.to().map_or(CRC_SEED, Self::crc);
        index_file.write_all(&crc.to_le_bytes())?;
        index_file.write_all(&offset.to_le_bytes())?;

        self.retire_header(old_offset)?;
        Ok(())
    }

    /// Marks a superseded header record as deleted and drops its text length so
    /// that packing leaves the text of the replacing record alone.
    fn retire_header(&self, offset: u64) -> crate::Result<()> {
        let header_path = self.file_name.with_extension(extensions::HEADER_DATA);
        let mut file = File::open(&header_path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut reader = BufReader::new(file);
        let mut old = JamMessageHeader::read(&mut reader)?;
        old.set_deleted(true);
        old.txt_len = 0;

        let mut file = OpenOptions::new()
            .write(true)
            .truncate(false)
            .open(&header_path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut writer = BufWriter::new(file);
        old.write(&mut writer)?;
        writer.flush()?;
        Ok(())
    }

    /// Sets the delete flag of a given message header.
    ///
    /// `read_header` will never return a deleted message, but it is still there
    /// and can be recovered until the message base gets packed.
    pub fn delete_message(&mut self, msg_number: u32) -> crate::Result<()> {
        if self.set_attributes(msg_number, attributes::MSG_DELETED, 0)? {
            self.header_info.active_msgs = self.header_info.active_msgs.saturating_sub(1);
            self.write_jhr_header()?;
        }
        Ok(())
    }

    /// Recovers a deleted message
    /// The opposite of `delete_message`
    pub fn restore_message(&mut self, msg_number: u32) -> crate::Result<()> {
        if self.set_attributes(msg_number, 0, attributes::MSG_DELETED)? {
            self.header_info.active_msgs = self.header_info.active_msgs.saturating_add(1);
            self.write_jhr_header()?;
        }
        Ok(())
    }

    pub fn read_last_read_file(&self) -> crate::Result<Vec<JamLastReadStorage>> {
        let last_read_file_name = self.file_name.with_extension(extensions::LASTREAD_INFO);
        let last_read_file = File::open(last_read_file_name)?;
        let mut res = Vec::new();
        let mut reader = BufReader::new(last_read_file);
        while let Ok(last_read) = JamLastReadStorage::load(&mut reader) {
            res.push(last_read);
        }
        Ok(res)
    }

    pub fn write_last_read(&self, opt: JamLastReadStorage) -> crate::Result<()> {
        let last_read_file_name = self.file_name.with_extension(extensions::LASTREAD_INFO);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(last_read_file_name)?;
        const LEN: u64 = 16;
        file.seek(SeekFrom::Start(self.last_read_record as u64 * LEN))?;
        opt.write(&mut file)?;
        Ok(())
    }

    pub fn create_last_read(
        &mut self,
        user_name_crc: u32,
        id: u32,
    ) -> crate::Result<JamLastReadStorage> {
        let opt = JamLastReadStorage {
            user_crc: user_name_crc,
            user_id: id,
            ..Default::default()
        };
        let last_read_file_name = self.file_name.with_extension(extensions::LASTREAD_INFO);

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(last_read_file_name)?;
        const LEN: u64 = 16;
        self.last_read_record = (file.metadata()?.len() / LEN) as i32;
        opt.write(&mut file)?;
        Ok(opt)
    }

    pub fn find_last_read(
        &mut self,
        user_name_crc: u32,
        id: u32,
    ) -> crate::Result<Option<JamLastReadStorage>> {
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
                return Ok(Some(JamLastReadStorage {
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

    /// Message numbers of every index record addressed to the given name CRC.
    ///
    /// The record number within the .JDX file plus `BaseMsgNum` is the message
    /// number, which is what this returns. Empty slots are skipped.
    pub fn search_message_index(&self, crc: u32) -> crate::Result<Vec<u32>> {
        let index_file_name = self.file_name.with_extension(extensions::MESSAGE_INDEX);
        let index_file = fs::read(index_file_name)?;

        if !index_file.len().is_multiple_of(INDEX_RECORD_SIZE) {
            return Err(JamError::IndexFileCorrupted.into());
        }
        let needle = crc.to_le_bytes();
        let empty = EMPTY_SLOT.to_le_bytes();
        let base = self.header_info.base_msg_num;
        let hit = move |(record, data): (usize, &[u8])| -> Option<u32> {
            if data[0..4] == needle && data[4..8] != empty {
                Some(base + record as u32)
            } else {
                None
            }
        };

        // All records have to be looked at, so large indices are worth splitting up.
        let res = if index_file.len() / INDEX_RECORD_SIZE < PARALLEL_SEARCH_THRESHOLD {
            index_file
                .chunks_exact(INDEX_RECORD_SIZE)
                .enumerate()
                .filter_map(hit)
                .collect()
        } else {
            index_file
                .par_chunks_exact(INDEX_RECORD_SIZE)
                .enumerate()
                .filter_map(hit)
                .collect()
        };
        Ok(res)
    }

    /// Iterates the message headers in the order they are stored in the .JHR file.
    ///
    /// Note that this walks the header file, so superseded and deleted records
    /// show up as well.
    pub fn iter(
        &self,
    ) -> crate::Result<impl Iterator<Item = crate::Result<JamMessageHeader>> + use<>> {
        let header_file_name = self.file_name.with_extension(extensions::HEADER_DATA);
        let mut f = File::open(header_file_name)?;
        let size = f.metadata()?.len();
        f.seek(SeekFrom::Start(JhrHeaderInfo::JHR_HEADER_SIZE))?;
        Ok(JamBaseMessageIter {
            reader: BufReader::new(f),
            size,
        })
    }
}

struct JamBaseMessageIter {
    reader: BufReader<File>,
    size: u64,
}

impl Iterator for JamBaseMessageIter {
    type Item = crate::Result<JamMessageHeader>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.reader.stream_position() {
            Ok(pos) if pos >= self.size => None,
            Ok(_) => Some(JamMessageHeader::read(&mut self.reader)),
            Err(err) => Some(Err(err.into())),
        }
    }
}

/// Releases the message base lock when dropped.
pub struct JamLockGuard<'a> {
    base: &'a JamMessageBase,
}

impl Drop for JamLockGuard<'_> {
    fn drop(&mut self) {
        self.base.unlock();
    }
}

/// Length of a file, treating a missing file as empty.
fn file_len(path: &Path) -> crate::Result<u64> {
    match fs::metadata(path) {
        Ok(meta) => Ok(meta.len()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(err) => Err(err.into()),
    }
}

/// Undoes a partial append so a failed write does not leave a torn record behind.
fn rollback(path: &Path, len: u64) {
    if let Err(err) = OpenOptions::new()
        .write(true)
        .truncate(false)
        .open(path)
        .and_then(|f| f.set_len(len))
    {
        log::error!(
            "Could not roll back {} to {len} bytes: {err}",
            path.display()
        );
    }
}

/// Used for writing messages to a JAM message base
/// It's more complex to create a valid jam message than it looks.
/// Using the builder pattern is recommended.
#[derive(Default)]
pub struct JamMessage {
    header: JamMessageHeader,
    text: BString,
}

impl JamMessage {
    pub fn message_number(&self) -> u32 {
        self.header.message_number
    }
    pub fn msgid_crc(&self) -> u32 {
        self.header.msgid_crc
    }

    /// Creates a new message with an unique message id
    pub fn new(msg_number: u32, aka: &EchomailAddress) -> Self {
        let now = SystemTime::now();
        let date_written = if let Ok(unix_time) = now.duration_since(UNIX_EPOCH) {
            unix_time.as_secs() as u32
        } else {
            0
        };

        let rnd: u32 = fastrand::u32(..);
        let id = BString::from(format!("{} {:08x}", aka, rnd));
        let msgid_crc = JamMessageBase::crc(&id);

        JamMessage {
            header: JamMessageHeader {
                message_number: msg_number,
                msgid_crc,
                date_written,
                sub_fields: vec![MessageSubfield::new(SubfieldType::MsgID, id)],
                ..Default::default()
            },
            text: BString::default(),
        }
    }

    pub fn with_reply_to(mut self, reply_to: u32) -> Self {
        self.header.reply_to = reply_to;
        self
    }

    /// Keeps the id a message arrived with instead of making up a new one, which
    /// is what lets two systems agree that they are looking at the same message.
    pub fn with_msg_id(mut self, id: BString) -> Self {
        self.header.msgid_crc = JamMessageBase::crc(&id);
        self.header
            .sub_fields
            .push(MessageSubfield::new(SubfieldType::MsgID, id));
        self
    }

    pub fn with_reply_id(mut self, id: BString) -> Self {
        self.header.reply_crc = JamMessageBase::crc(&id);
        self.header
            .sub_fields
            .push(MessageSubfield::new(SubfieldType::ReplyID, id));
        self
    }

    pub fn with_date_time(mut self, time: DateTime<Utc>) -> Self {
        self.header.date_written = time.timestamp() as u32;
        self.header.sub_fields.push(MessageSubfield::new(
            SubfieldType::DateWritten,
            BString::from(time.to_rfc3339()),
        ));
        self
    }

    pub fn with_packout_date(mut self, time: DateTime<Utc>) -> Self {
        self.header.sub_fields.push(MessageSubfield::new(
            SubfieldType::PackoutDate,
            BString::from(time.to_rfc3339()),
        ));
        self
    }

    pub fn with_text(mut self, text: BString) -> Self {
        self.text = text;
        self
    }
    pub fn with_attributes(mut self, attributes: u32) -> Self {
        self.header.attributes = attributes;
        self
    }

    pub fn with_password(mut self, password: &BString) -> Self {
        self.header.password_crc = JamMessageBase::crc(password);
        self
    }

    pub fn with_from(mut self, name: BString) -> Self {
        self.header
            .sub_fields
            .push(MessageSubfield::new(SubfieldType::SenderName, name));
        self
    }

    pub fn with_to(mut self, name: BString) -> Self {
        self.header
            .sub_fields
            .push(MessageSubfield::new(SubfieldType::RecvName, name));
        self
    }

    pub fn with_subject(mut self, subject: BString) -> Self {
        self.header
            .sub_fields
            .push(MessageSubfield::new(SubfieldType::Subject, subject));
        self
    }

    pub fn with_sub_field(mut self, sub_field: MessageSubfield) -> Self {
        self.header.sub_fields.push(sub_field);
        self
    }

    pub fn with_is_deleted(mut self, deleted: bool) -> Self {
        if deleted {
            self.header.attributes |= attributes::MSG_DELETED;
        } else {
            self.header.attributes &= !attributes::MSG_DELETED;
        }
        self
    }

    pub fn text(&self) -> &BString {
        &self.text
    }

    pub(crate) fn create_jam_header(&self) -> JamMessageHeader {
        self.header.clone()
    }

    pub fn reply_to(&self) -> u32 {
        self.header.reply_to
    }

    pub fn reply_first(&self) -> u32 {
        self.header.reply_first
    }

    pub fn reply_next(&self) -> u32 {
        self.header.reply_next
    }

    pub fn from(&self) -> Option<&BString> {
        self.header.from()
    }

    pub fn to(&self) -> Option<&BString> {
        self.header.to()
    }

    pub fn set_reply_crc(&mut self, crc: u32) {
        self.header.reply_crc = crc;
    }

    pub fn set_reply_first(&mut self, reply_first: u32) {
        self.header.reply_first = reply_first;
    }

    pub fn set_reply_next(&mut self, reply_next: u32) {
        self.header.reply_next = reply_next;
    }

    pub(crate) fn is_deleted(&self) -> bool {
        self.header.is_deleted()
    }
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
