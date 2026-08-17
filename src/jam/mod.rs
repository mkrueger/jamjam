use std::fs::{self, OpenOptions};
use std::io::{BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
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
pub mod raw;
pub mod verify;

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

    #[error("Index record {0} is half empty")]
    InvalidIndexRecord(u64),

    #[error("Index record for message {0} points to a header numbered {1}")]
    IndexMessageNumberMismatch(u32, u32),

    #[error("Message base is full, offset {0} does not fit the 32 bit field the format provides")]
    MessageBaseFull(u64),

    #[error("Message numbers are exhausted")]
    MessageNumbersExhausted,

    #[error("Message text of {0} bytes exceeds the 4 GB a JAM message base can address")]
    MessageTooLarge(usize),

    #[error("Message text at offset {0} runs {1} bytes past the end of the text file")]
    TextOutOfBounds(u64, u64),

    #[error("Lastread file corrupted, {0} bytes is not a whole number of records")]
    LastReadFileCorrupted(u64),

    #[error(
        "a shared message base lock cannot be upgraded, take the exclusive lock before reading"
    )]
    LockUpgrade,

    #[error("subfield block of {0} bytes exceeds the {1} byte limit")]
    SubfieldBlockTooLarge(u32, u32),

    #[error("subfield block truncated, expected {0} bytes, got {1}")]
    SubfieldBlockTruncated(u32, usize),

    #[error("subfield of {0} bytes exceeds the {1} byte limit")]
    SubfieldTooLarge(u32, u32),
}

/// JAM stores every file offset in a `ulong`, so a base cannot grow past 4 GB.
fn offset_u32(value: u64) -> crate::Result<u32> {
    u32::try_from(value).map_err(|_| JamError::MessageBaseFull(value).into())
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

    /// Stable advisory lock, never replaced while the base is open.
    pub const LOCK_FILE: &str = "jamlock";
}

const JAM_SIGNATURE: [u8; 4] = [b'J', b'A', b'M', 0];

/// Two ulongs: CRC-32 of the recipient and the header offset in the .JHR file.
pub(crate) const INDEX_RECORD_SIZE: usize = 8;

/// Four ulongs: user CRC, user id, last read and high read message number.
pub(crate) const LASTREAD_RECORD_SIZE: usize = 16;

/// "If the lastread record is deleted, UserCRC and UserID are both set to -1."
pub(crate) const LASTREAD_DELETED: [u8; 8] = [0xFF; 8];

/// Below this many index records a plain scan beats spawning rayon tasks.
const PARALLEL_SEARCH_THRESHOLD: usize = 8192;

/// Which kind of advisory lock the base currently holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LockMode {
    /// Several programs may read at the same time.
    Shared,
    /// One program may mutate, nobody else may read or mutate.
    Exclusive,
}

pub struct JamMessageBase {
    file_name: PathBuf,
    header_info: JhrHeaderInfo,
    /// Number of records in the .JDX file, including the slots of deleted messages.
    index_records: u32,
    /// Held open for the whole lifetime, the .JHR file is the lock token.
    lock_file: File,
    lock_depth: u32,
    lock_mode: LockMode,
}

impl JamMessageBase {
    /// opens an existing message base with base path (without any extension)
    pub fn open<P: AsRef<Path>>(file_name: P) -> crate::Result<Self> {
        let file_name = file_name.as_ref();

        let lock_path = file_name.with_extension(extensions::LOCK_FILE);
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(lock_path)?;
        lock_file.lock()?;

        // Recovery and initial state loading must see one exclusive generation.
        pack::recover_interrupted_pack(file_name)?;

        let header_file_name = file_name.with_extension(extensions::HEADER_DATA);
        let mut file = File::open(&header_file_name).inspect_err(|err| {
            log::error!("Error opening message base {}: {err}", file_name.display());
        })?;
        let header_info = JhrHeaderInfo::load(&mut file)?;
        let mut base = Self {
            file_name: file_name.into(),
            header_info,
            index_records: 0,
            lock_file,
            lock_depth: 1,
            lock_mode: LockMode::Exclusive,
        };
        base.index_records = base.count_index_records()?;
        base.unlock();
        Ok(base)
    }

    fn refresh_state(&mut self) -> crate::Result<()> {
        let header_file_name = self.file_name.with_extension(extensions::HEADER_DATA);
        let mut header = File::open(header_file_name)?;
        self.header_info = JhrHeaderInfo::load(&mut header)?;
        self.index_records = self.count_index_records()?;
        Ok(())
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
        let file_name = file_name.as_ref();
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(file_name.with_extension(extensions::LOCK_FILE))?;
        lock_file.lock()?;

        let header_path = file_name.with_extension(extensions::HEADER_DATA);
        let text_path = file_name.with_extension(extensions::TEXT_DATA);
        let index_path = file_name.with_extension(extensions::MESSAGE_INDEX);
        let lastread_path = file_name.with_extension(extensions::LASTREAD_INFO);
        let mut created = Vec::new();
        let result: crate::Result<()> = (|| {
            JhrHeaderInfo::create(&header_path, passwordcrc)?;
            created.push(header_path.clone());
            for path in [&text_path, &index_path, &lastread_path] {
                OpenOptions::new().create_new(true).write(true).open(path)?;
                created.push(path.clone());
            }
            Ok(())
        })();

        if result.is_err() {
            for path in created.iter().rev() {
                let _ = fs::remove_file(path);
            }
        }
        let unlock_result = lock_file.unlock();
        drop(lock_file);
        result?;
        unlock_result?;
        Self::open(file_name)
    }

    pub fn delete_message_base(mut self) -> crate::Result<()> {
        self.lock()?;
        let file_name = self.file_name.clone();
        for extension in [
            extensions::HEADER_DATA,
            extensions::TEXT_DATA,
            extensions::MESSAGE_INDEX,
            extensions::LASTREAD_INFO,
        ] {
            fs::remove_file(file_name.with_extension(extension))?;
        }
        self.unlock();
        Ok(())
    }

    /// Flushes the message base files to disk.
    ///
    /// Writes are not synced on every message, so call this when a batch has to
    /// survive a power cut.
    pub fn sync(&self) -> crate::Result<()> {
        for extension in [
            extensions::TEXT_DATA,
            extensions::HEADER_DATA,
            extensions::MESSAGE_INDEX,
            extensions::LASTREAD_INFO,
        ] {
            let path = self.file_name.with_extension(extension);
            match OpenOptions::new().write(true).truncate(false).open(&path) {
                Ok(file) => file.sync_all()?,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
        pack::sync_dir(&self.file_name);
        Ok(())
    }

    /// Takes the exclusive advisory lock that guards the message base.
    ///
    /// JAM bases are shared between programs, so this is a real file lock and
    /// not just a flag. Every mutating call takes it on its own; use this to
    /// hold it across a group of them. Calls nest, and the lock is released
    /// when the outermost one is undone. Readers that only need a consistent
    /// snapshot should use [`Self::lock_shared`] instead.
    pub fn lock(&mut self) -> crate::Result<()> {
        self.acquire(LockMode::Exclusive, true).map(|_| ())
    }

    /// Takes a shared lock, which other readers may hold at the same time.
    ///
    /// Writers are excluded for as long as any reader holds it. A shared lock
    /// cannot be upgraded, so mutating calls made while one is held fail with
    /// [`JamError::LockUpgrade`] instead of writing without exclusive access.
    pub fn lock_shared(&mut self) -> crate::Result<()> {
        self.acquire(LockMode::Shared, true).map(|_| ())
    }

    /// Releases one level of [`Self::lock`] or [`Self::lock_shared`].
    pub fn unlock(&mut self) {
        if self.lock_depth == 0 {
            return;
        }
        self.lock_depth -= 1;
        if self.lock_depth == 0
            && let Err(err) = self.lock_file.unlock()
        {
            log::error!("Could not release the message base lock: {err}");
        }
    }

    /// Takes the lock without waiting, returning false if another program holds it.
    pub fn try_lock(&mut self) -> crate::Result<bool> {
        self.acquire(LockMode::Exclusive, false)
    }

    /// Takes the shared lock without waiting, returning false if a writer holds it.
    pub fn try_lock_shared(&mut self) -> crate::Result<bool> {
        self.acquire(LockMode::Shared, false)
    }

    /// Takes the lock in the given mode, refreshing the cached state when the
    /// outermost level is entered.
    fn acquire(&mut self, mode: LockMode, blocking: bool) -> crate::Result<bool> {
        if self.lock_depth > 0 {
            // A shared lock is held by other readers as well, so the file lock
            // cannot be turned into an exclusive one without dropping it first.
            if mode == LockMode::Exclusive && self.lock_mode == LockMode::Shared {
                return Err(JamError::LockUpgrade.into());
            }
        } else {
            if !self.take_file_lock(mode, blocking)? {
                return Ok(false);
            }
            if let Err(err) = self.refresh_state() {
                let _ = self.lock_file.unlock();
                return Err(err);
            }
            self.lock_mode = mode;
        }
        self.lock_depth = self
            .lock_depth
            .checked_add(1)
            .ok_or_else(|| crate::Error::jam(0, "message base lock depth overflow"))?;
        Ok(true)
    }

    fn take_file_lock(&self, mode: LockMode, blocking: bool) -> crate::Result<bool> {
        if blocking {
            match mode {
                LockMode::Shared => self.lock_file.lock_shared()?,
                LockMode::Exclusive => self.lock_file.lock()?,
            }
            return Ok(true);
        }
        let result = match mode {
            LockMode::Shared => self.lock_file.try_lock_shared(),
            LockMode::Exclusive => self.lock_file.try_lock(),
        };
        match result {
            Ok(()) => Ok(true),
            Err(std::fs::TryLockError::WouldBlock) => Ok(false),
            Err(std::fs::TryLockError::Error(err)) => Err(err.into()),
        }
    }

    /// Runs a closure with the message base locked exclusively.
    pub fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> crate::Result<T>,
    ) -> crate::Result<T> {
        self.lock()?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
        self.unlock();
        match result {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    /// Runs a closure with the message base locked for reading.
    ///
    /// Writers are kept out for the duration, so a sequence of reads sees one
    /// consistent generation of the base.
    pub fn read_transaction<T>(
        &mut self,
        f: impl FnOnce(&Self) -> crate::Result<T>,
    ) -> crate::Result<T> {
        self.lock_shared()?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
        self.unlock();
        match result {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }
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
        self.transaction(|base| {
            let text_path = base.file_name.with_extension(extensions::TEXT_DATA);
            let header_path = base.file_name.with_extension(extensions::HEADER_DATA);
            let index_path = base.file_name.with_extension(extensions::MESSAGE_INDEX);

            let text_len = file_len(&text_path)?;
            let header_len = file_len(&header_path)?;
            let index_len = file_len(&index_path)?;
            let original_header_info = base.header_info.clone();
            let original_index_records = base.index_records;

            let result = (|| {
                let msg_number = base
                    .highest_message_number()
                    .checked_add(1)
                    .ok_or(JamError::MessageNumbersExhausted)?;
                Self::append_message(
                    message,
                    msg_number,
                    &text_path,
                    &header_path,
                    &index_path,
                    text_len,
                    header_len,
                )?;
                base.index_records = base
                    .index_records
                    .checked_add(1)
                    .ok_or(JamError::MessageNumbersExhausted)?;
                if !message.is_deleted() {
                    base.header_info.active_msgs = base
                        .header_info
                        .active_msgs
                        .checked_add(1)
                        .ok_or(JamError::MessageNumbersExhausted)?;
                }
                base.write_jhr_header()?;
                Ok(msg_number)
            })();

            match result {
                Ok(msg_number) => Ok(msg_number),
                Err(err) => {
                    rollback(&text_path, text_len);
                    rollback(&header_path, header_len);
                    rollback(&index_path, index_len);
                    base.header_info = original_header_info;
                    base.index_records = original_index_records;
                    if let Err(restore_err) = base.store_jhr_header() {
                        log::error!("Could not restore the JAM base header after a failed write: {restore_err}");
                    }
                    Err(err)
                }
            }
        })
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
        header.offset = offset_u32(text_offset)?;
        header.txt_len = u32::try_from(message.text().len())
            .map_err(|_| JamError::MessageTooLarge(message.text().len()))?;
        offset_u32(text_offset + header.txt_len as u64)?;
        let index_offset = offset_u32(header_offset)?;

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
        index_file.write_all(&index_offset.to_le_bytes())?;
        index_file.flush()?;
        Ok(())
    }

    /// Writes the current header to disk.
    pub fn write_jhr_header(&mut self) -> crate::Result<()> {
        self.transaction(|base| base.write_jhr_header_locked())
    }

    fn write_jhr_header_locked(&mut self) -> crate::Result<()> {
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

    fn store_jhr_header(&self) -> crate::Result<()> {
        let header_path = self.file_name.with_extension(extensions::HEADER_DATA);
        let header_file = OpenOptions::new()
            .write(true)
            .truncate(false)
            .open(header_path)?;
        let mut writer = BufWriter::new(header_file);
        self.header_info.store(&mut writer)?;
        writer.flush()?;
        Ok(())
    }

    /// Updates header with the one from disk.
    /// Usually it's not required to call that (only for outside changes detected)
    pub fn read_jhr_header(&mut self) -> crate::Result<()> {
        self.refresh_state()
    }

    pub fn read_message_text(&self, header: &JamMessageHeader) -> crate::Result<BString> {
        let text_file_name = self.file_name.with_extension(extensions::TEXT_DATA);
        let mut text_file = File::open(text_file_name)?;
        // The length comes off disk, so check it before trusting it with an allocation.
        let file_len = text_file.metadata()?.len();
        let end = header.offset as u64 + header.txt_len as u64;
        if end > file_len {
            return Err(JamError::TextOutOfBounds(header.offset as u64, end - file_len).into());
        }
        text_file.seek(SeekFrom::Start(header.offset as u64))?;
        let mut buffer = vec![0; header.txt_len as usize];
        text_file.read_exact(&mut buffer)?;
        Ok(BString::new(buffer))
    }

    /// Iterates live messages in message-number order through the .JDX index.
    ///
    /// Empty and deleted slots are skipped. Physical JHR traversal, including
    /// retired records, is available in [`raw::physical_headers`].
    ///
    /// The walk is not locked. Take [`Self::lock_shared`] or
    /// [`Self::read_transaction`] if a writer may be packing or appending.
    pub fn messages(&self) -> impl Iterator<Item = crate::Result<JamMessageHeader>> + use<'_> {
        (self.lowest_message_number()..=self.highest_message_number()).filter_map(move |number| {
            match self.read_header(number) {
                Ok(header) => Some(Ok(header)),
                Err(crate::Error::Jam(JamError::MessageDeleted)) => None,
                Err(err) => Some(Err(err)),
            }
        })
    }

    /// Live messages with their text, in message-number order.
    ///
    /// Like [`Self::messages`], this is not locked on its own.
    pub fn messages_full(&self) -> impl Iterator<Item = crate::Result<JamMessage>> + use<'_> {
        self.messages().map(|header| {
            let header = header?;
            let text = self.read_message_text(&header)?;
            Ok(JamMessage::from_stored(header, text))
        })
    }

    /// Reads a live message and the text that belongs to it.
    pub fn read_message(&self, msg_number: u32) -> crate::Result<JamMessage> {
        let header = self.read_header(msg_number)?;
        let text = self.read_message_text(&header)?;
        Ok(JamMessage::from_stored(header, text))
    }

    pub fn read_header(&self, msg_number: u32) -> crate::Result<JamMessageHeader> {
        let offset = self.header_offset(msg_number)?;

        let header_file_name = self.file_name.with_extension(extensions::HEADER_DATA);
        let mut header_file = File::open(header_file_name)?;
        header_file.seek(SeekFrom::Start(offset))?;
        let mut reader = BufReader::new(header_file);
        let header = JamMessageHeader::read(&mut reader)?;

        if header.message_number != msg_number {
            return Err(
                JamError::IndexMessageNumberMismatch(msg_number, header.message_number).into(),
            );
        }

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
    pub(crate) fn set_attributes(
        &mut self,
        msg_number: u32,
        set: u32,
        clear: u32,
    ) -> crate::Result<bool> {
        self.transaction(|base| base.set_attributes_locked(msg_number, set, clear))
    }

    fn set_attributes_locked(
        &mut self,
        msg_number: u32,
        set: u32,
        clear: u32,
    ) -> crate::Result<bool> {
        let offset = self.header_offset(msg_number)?;
        let old_header = self.read_header_at(offset)?;
        let attributes = (old_header.attributes | set) & !clear;
        if attributes == old_header.attributes {
            return Ok(false);
        }

        let original_info = self.header_info.clone();
        let old_deleted = old_header.is_deleted();
        let mut new_header = old_header.clone();
        new_header.attributes = attributes;

        let result = (|| {
            self.write_header_at(offset, &new_header)?;
            self.adjust_active_messages(old_deleted, new_header.is_deleted())?;
            self.write_jhr_header()
        })();

        if let Err(err) = result {
            self.header_info = original_info;
            if let Err(restore_err) = self.write_header_at(offset, &old_header) {
                log::error!(
                    "Could not restore a JAM header after an attribute update failed: {restore_err}"
                );
            }
            if let Err(restore_err) = self.store_jhr_header() {
                log::error!(
                    "Could not restore the JAM base header after an attribute update failed: {restore_err}"
                );
            }
            return Err(err);
        }

        Ok(true)
    }

    /// Replaces a message header. The subfields can change length, so the new
    /// record goes to the end of the header file and the index is pointed at it.
    ///
    /// As the format requires, the old record is marked deleted and its text
    /// length is zeroed so that packing does not reclaim the text that now
    /// belongs to the new record.
    pub(crate) fn update_header(
        &mut self,
        msg_number: u32,
        header: &JamMessageHeader,
    ) -> crate::Result<()> {
        self.transaction(|base| base.update_header_locked(msg_number, header))
    }

    fn update_header_locked(
        &mut self,
        msg_number: u32,
        header: &JamMessageHeader,
    ) -> crate::Result<()> {
        let old_offset = self.header_offset(msg_number)?;
        let record = self.index_record(msg_number)?;
        let old_header = self.read_header_at(old_offset)?;
        let original_info = self.header_info.clone();

        let header_path = self.file_name.with_extension(extensions::HEADER_DATA);
        let original_header_len = file_len(&header_path)?;
        let index_path = self.file_name.with_extension(extensions::MESSAGE_INDEX);
        let original_index = read_index_record(&index_path, record)?;

        let mut replacement = header.clone();
        replacement.message_number = msg_number;

        let result = (|| {
            let header_file = OpenOptions::new().append(true).open(&header_path)?;
            let offset = offset_u32(header_file.metadata()?.len())?;
            let mut writer = BufWriter::new(header_file);
            replacement.write(&mut writer)?;
            writer.flush()?;

            write_index_record(&index_path, record, &replacement, offset)?;
            self.retire_header(old_offset)?;
            self.adjust_active_messages(old_header.is_deleted(), replacement.is_deleted())?;
            self.write_jhr_header()
        })();

        if let Err(err) = result {
            rollback(&header_path, original_header_len);
            self.header_info = original_info;
            if let Err(restore_err) = self.write_header_at(old_offset, &old_header) {
                log::error!("Could not restore a superseded JAM header: {restore_err}");
            }
            if let Err(restore_err) = restore_index_record(&index_path, record, original_index) {
                log::error!("Could not restore a JAM index record: {restore_err}");
            }
            if let Err(restore_err) = self.store_jhr_header() {
                log::error!(
                    "Could not restore the JAM base header after an update failed: {restore_err}"
                );
            }
            return Err(err);
        }
        Ok(())
    }

    fn read_header_at(&self, offset: u64) -> crate::Result<JamMessageHeader> {
        let header_path = self.file_name.with_extension(extensions::HEADER_DATA);
        let mut file = BufReader::new(File::open(header_path)?);
        file.seek(SeekFrom::Start(offset))?;
        JamMessageHeader::read(&mut file)
    }

    fn write_header_at(&self, offset: u64, header: &JamMessageHeader) -> crate::Result<()> {
        let header_path = self.file_name.with_extension(extensions::HEADER_DATA);
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(false)
            .open(header_path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut writer = BufWriter::new(file);
        header.write(&mut writer)?;
        writer.flush()?;
        Ok(())
    }

    fn adjust_active_messages(&mut self, was_deleted: bool, is_deleted: bool) -> crate::Result<()> {
        match (was_deleted, is_deleted) {
            (false, true) => {
                self.header_info.active_msgs = self
                    .header_info
                    .active_msgs
                    .checked_sub(1)
                    .ok_or_else(|| crate::Error::jam(0, "ActiveMsgs underflow"))?;
            }
            (true, false) => {
                self.header_info.active_msgs = self
                    .header_info
                    .active_msgs
                    .checked_add(1)
                    .ok_or_else(|| crate::Error::jam(0, "ActiveMsgs overflow"))?;
            }
            _ => {}
        }
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
        self.set_attributes(msg_number, attributes::MSG_DELETED, 0)
            .map(|_| ())
    }

    /// Recovers a deleted message
    /// The opposite of `delete_message`
    pub fn restore_message(&mut self, msg_number: u32) -> crate::Result<()> {
        self.set_attributes(msg_number, 0, attributes::MSG_DELETED)
            .map(|_| ())
    }

    /// All lastread records, deleted ones included.
    pub fn read_last_read_file(&self) -> crate::Result<Vec<JamLastReadStorage>> {
        let data = self.read_last_read_records()?;
        data.chunks_exact(LASTREAD_RECORD_SIZE)
            .map(|mut record| JamLastReadStorage::load(&mut record))
            .collect()
    }

    fn read_last_read_records(&self) -> crate::Result<Vec<u8>> {
        let path = self.file_name.with_extension(extensions::LASTREAD_INFO);
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err.into()),
        };
        if !data.len().is_multiple_of(LASTREAD_RECORD_SIZE) {
            return Err(JamError::LastReadFileCorrupted(data.len() as u64).into());
        }
        Ok(data)
    }

    /// Record a user occupies in the .JLR file, deleted records are skipped.
    fn last_read_record(&self, user_crc: u32, user_id: u32) -> crate::Result<Option<u64>> {
        let data = self.read_last_read_records()?;
        Ok(find_last_read_record(&data, user_crc, user_id).map(|record| record as u64))
    }

    /// Replaces the lastread file in one step, so an interrupted update cannot
    /// leave a half written record behind.
    pub(crate) fn store_last_read_records(&self, data: &[u8]) -> crate::Result<()> {
        let path = self.file_name.with_extension(extensions::LASTREAD_INFO);
        let mut tmp = path.as_os_str().to_os_string();
        tmp.push(".writing");
        let tmp = PathBuf::from(tmp);
        fs::write(&tmp, data)?;
        pack::sync_file(&tmp)?;
        fs::rename(&tmp, &path)?;
        pack::sync_dir(&self.file_name);
        Ok(())
    }

    /// Stores a lastread record, replacing the one the user already has.
    ///
    /// The format does not fix the position of a record, so it is searched for
    /// on every store, as the specification requires. A user without a record
    /// yet gets one appended.
    pub fn write_last_read(&mut self, storage: &JamLastReadStorage) -> crate::Result<()> {
        self.transaction(|base| {
            let mut data = base.read_last_read_records()?;
            let mut record = Vec::with_capacity(LASTREAD_RECORD_SIZE);
            storage.write(&mut record)?;
            match find_last_read_record(&data, storage.user_crc, storage.user_id) {
                Some(position) => {
                    let start = position * LASTREAD_RECORD_SIZE;
                    data[start..start + LASTREAD_RECORD_SIZE].copy_from_slice(&record);
                }
                None => data.extend_from_slice(&record),
            }
            base.store_last_read_records(&data)
        })
    }

    /// Returns the lastread record of a user, creating an empty one if the user
    /// does not have one yet.
    pub fn create_last_read(
        &mut self,
        user_crc: u32,
        user_id: u32,
    ) -> crate::Result<JamLastReadStorage> {
        self.transaction(|base| {
            if let Some(existing) = base.find_last_read(user_crc, user_id)? {
                return Ok(existing);
            }
            let storage = JamLastReadStorage {
                user_crc,
                user_id,
                ..Default::default()
            };
            base.write_last_read(&storage)?;
            Ok(storage)
        })
    }

    /// Looks up the lastread record of a user.
    pub fn find_last_read(
        &self,
        user_crc: u32,
        user_id: u32,
    ) -> crate::Result<Option<JamLastReadStorage>> {
        let Some(record) = self.last_read_record(user_crc, user_id)? else {
            return Ok(None);
        };
        let data = self.read_last_read_records()?;
        let start = record as usize * LASTREAD_RECORD_SIZE;
        let mut slice = &data[start..start + LASTREAD_RECORD_SIZE];
        Ok(Some(JamLastReadStorage::load(&mut slice)?))
    }

    /// Marks the lastread record of a user deleted, as the format prescribes.
    pub fn delete_last_read(&mut self, user_crc: u32, user_id: u32) -> crate::Result<bool> {
        self.transaction(|base| {
            let mut data = base.read_last_read_records()?;
            let Some(position) = find_last_read_record(&data, user_crc, user_id) else {
                return Ok(false);
            };
            let start = position * LASTREAD_RECORD_SIZE;
            data[start..start + LASTREAD_DELETED.len()].copy_from_slice(&LASTREAD_DELETED);
            base.store_last_read_records(&data)?;
            Ok(true)
        })
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

    /// Message numbers addressed to the given recipient name.
    pub fn search_to(&self, name: &BString) -> crate::Result<Vec<u32>> {
        self.search_message_index(Self::crc(name))
    }

    /// Message numbers whose MSGID CRC matches `id`.
    pub fn find_by_msgid(&self, id: &BString) -> crate::Result<Vec<u32>> {
        self.find_by_msgid_crc(Self::crc(id))
    }

    /// Message numbers whose stored MSGID CRC matches `crc`.
    pub fn find_by_msgid_crc(&self, crc: u32) -> crate::Result<Vec<u32>> {
        self.messages()
            .filter_map(|header| match header {
                Ok(header) if header.msgid_crc == crc => Some(Ok(header.message_number)),
                Ok(_) => None,
                Err(err) => Some(Err(err)),
            })
            .collect()
    }

    pub(crate) fn read_physical_headers(&self) -> crate::Result<Vec<JamMessageHeader>> {
        self.physical_headers()?.collect()
    }

    pub(crate) fn physical_headers(
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

/// Length of a file, treating a missing file as empty.
pub(crate) fn file_len(path: &Path) -> crate::Result<u64> {
    match fs::metadata(path) {
        Ok(meta) => Ok(meta.len()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(err) => Err(err.into()),
    }
}

/// Position of a user's lastread record, skipping the deleted ones.
fn find_last_read_record(data: &[u8], user_crc: u32, user_id: u32) -> Option<usize> {
    let mut needle = [0; 8];
    needle[..4].copy_from_slice(&user_crc.to_le_bytes());
    needle[4..].copy_from_slice(&user_id.to_le_bytes());
    if needle == LASTREAD_DELETED {
        return None;
    }
    data.chunks_exact(LASTREAD_RECORD_SIZE)
        .position(|record| record[..8] == needle)
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

fn read_index_record(path: &Path, record: u64) -> crate::Result<[u8; INDEX_RECORD_SIZE]> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(record * INDEX_RECORD_SIZE as u64))?;
    let mut data = [0; INDEX_RECORD_SIZE];
    file.read_exact(&mut data)?;
    Ok(data)
}

fn restore_index_record(
    path: &Path,
    record: u64,
    data: [u8; INDEX_RECORD_SIZE],
) -> crate::Result<()> {
    let mut file = OpenOptions::new().write(true).truncate(false).open(path)?;
    file.seek(SeekFrom::Start(record * INDEX_RECORD_SIZE as u64))?;
    file.write_all(&data)?;
    Ok(())
}

fn write_index_record(
    path: &Path,
    record: u64,
    header: &JamMessageHeader,
    offset: u32,
) -> crate::Result<()> {
    let mut file = OpenOptions::new().write(true).truncate(false).open(path)?;
    file.seek(SeekFrom::Start(record * INDEX_RECORD_SIZE as u64))?;
    let crc = header.to().map_or(CRC_SEED, JamMessageBase::crc);
    file.write_all(&crc.to_le_bytes())?;
    file.write_all(&offset.to_le_bytes())?;
    Ok(())
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
    pub fn msgid_crc(&self) -> u32 {
        self.header.msgid_crc
    }

    /// Rebuilds a message from a stored header and its text.
    pub fn from_stored(header: JamMessageHeader, text: BString) -> Self {
        Self { header, text }
    }

    /// Creates a new message with an unique message id
    pub fn new(aka: &EchomailAddress) -> Self {
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

    pub fn header(&self) -> &JamMessageHeader {
        &self.header
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
