//! Consistency checking and repair of a JAM message base.
//!
//! [`JamMessageBase::verify`] never writes, so it is safe to run against a live
//! base. [`JamMessageBase::repair`] rewrites the index and the derived counters
//! and has to be asked for explicitly.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{BufReader, Seek, SeekFrom};

use super::{
    INDEX_RECORD_SIZE, JamMessageBase, LASTREAD_DELETED, LASTREAD_RECORD_SIZE, extensions,
    jhr_header::JhrHeaderInfo, msg_header::JamMessageHeader, pack::EMPTY_SLOT,
};

/// Something that does not match what the format guarantees.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Problem {
    /// The .JDX file does not consist of whole records.
    IndexFileTruncated { bytes: u64 },
    /// One half of an index record marks an empty slot, the other does not.
    IndexRecordHalfEmpty { message_number: u32 },
    /// An index record points outside the .JHR file.
    IndexOffsetOutOfBounds { message_number: u32, offset: u32 },
    /// The header an index record points at could not be read.
    UnreadableHeader { message_number: u32, reason: String },
    /// The header an index record points at carries a different number.
    MessageNumberMismatch { message_number: u32, found: u32 },
    /// Two index records point at the same header.
    DuplicateHeaderOffset {
        message_number: u32,
        other: u32,
        offset: u32,
    },
    /// A header addresses text that the .JDT file does not hold.
    TextOutOfBounds {
        message_number: u32,
        offset: u32,
        length: u32,
    },
    /// `ActiveMsgs` in the base header disagrees with the index.
    ActiveMessageCountWrong { stored: u32, counted: u32 },
    /// Bytes in the .JDT file that no indexed header refers to.
    UnreferencedText { bytes: u64 },
    /// The .JLR file does not consist of whole records.
    LastReadFileTruncated { bytes: u64 },
    /// A lastread pointer names a message the base cannot address.
    LastReadOutOfRange {
        user_crc: u32,
        user_id: u32,
        last_read_msg: u32,
        high_read_msg: u32,
    },
}

impl Problem {
    /// Whether rebuilding the index from the headers can resolve this.
    fn fixed_by_reindex(&self) -> bool {
        matches!(
            self,
            Problem::IndexFileTruncated { .. }
                | Problem::IndexRecordHalfEmpty { .. }
                | Problem::IndexOffsetOutOfBounds { .. }
                | Problem::MessageNumberMismatch { .. }
                | Problem::DuplicateHeaderOffset { .. }
                | Problem::ActiveMessageCountWrong { .. }
        )
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::IndexFileTruncated { bytes } => {
                write!(
                    f,
                    "index file of {bytes} bytes is not a whole number of records"
                )
            }
            Problem::IndexRecordHalfEmpty { message_number } => {
                write!(f, "index record of message {message_number} is half empty")
            }
            Problem::IndexOffsetOutOfBounds {
                message_number,
                offset,
            } => write!(
                f,
                "index record of message {message_number} points past the header file at offset {offset}"
            ),
            Problem::UnreadableHeader {
                message_number,
                reason,
            } => write!(
                f,
                "header of message {message_number} is unreadable: {reason}"
            ),
            Problem::MessageNumberMismatch {
                message_number,
                found,
            } => write!(
                f,
                "index record of message {message_number} points at a header numbered {found}"
            ),
            Problem::DuplicateHeaderOffset {
                message_number,
                other,
                offset,
            } => write!(
                f,
                "messages {other} and {message_number} share the header at offset {offset}"
            ),
            Problem::TextOutOfBounds {
                message_number,
                offset,
                length,
            } => write!(
                f,
                "message {message_number} claims {length} bytes of text at offset {offset}, past the end of the text file"
            ),
            Problem::ActiveMessageCountWrong { stored, counted } => {
                write!(
                    f,
                    "base header counts {stored} active messages, the index holds {counted}"
                )
            }
            Problem::UnreferencedText { bytes } => {
                write!(
                    f,
                    "{bytes} bytes of the text file belong to no indexed message"
                )
            }
            Problem::LastReadFileTruncated { bytes } => write!(
                f,
                "lastread file of {bytes} bytes is not a whole number of records"
            ),
            Problem::LastReadOutOfRange {
                user_crc,
                user_id,
                last_read_msg,
                high_read_msg,
            } => write!(
                f,
                "lastread record of user {user_id} (crc {user_crc}) points at messages {last_read_msg}/{high_read_msg} that the base does not hold"
            ),
        }
    }
}

/// What [`JamMessageBase::verify`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct VerifyReport {
    /// Records in the .JDX file, empty slots included.
    pub index_records: u32,
    /// Indexed messages that are neither empty nor deleted.
    pub live_messages: u32,
    pub problems: Vec<Problem>,
}

impl VerifyReport {
    /// True when the base matches what the format guarantees.
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }

    fn needs_reindex(&self) -> bool {
        self.problems.iter().any(Problem::fixed_by_reindex)
    }
}

impl fmt::Display for VerifyReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_ok() {
            return write!(
                f,
                "{} live messages in {} index records, no problems found",
                self.live_messages, self.index_records
            );
        }
        writeln!(f, "{} problems found:", self.problems.len())?;
        for problem in &self.problems {
            writeln!(f, "  {problem}")?;
        }
        Ok(())
    }
}

/// What [`JamMessageBase::repair`] changed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RepairReport {
    /// The state the base was in before the repair.
    pub before: VerifyReport,
    /// The state the base is in afterwards; problems left here need a human.
    pub after: VerifyReport,
    /// Whether the index was rebuilt from the headers.
    pub reindexed: bool,
    /// Bytes dropped from a trailing partial lastread record.
    pub last_read_bytes_dropped: u64,
    /// Lastread pointers that were pulled back into the message number range.
    pub last_read_records_clamped: u32,
}

impl JamMessageBase {
    /// Checks the base against the format without changing anything.
    ///
    /// The base is locked for reading while it is walked, so the report
    /// describes one consistent generation of the files.
    pub fn verify(&mut self) -> crate::Result<VerifyReport> {
        self.read_transaction(|base| base.verify_locked())
    }

    /// Rebuilds what can be derived again and reports what is left.
    ///
    /// The index is rebuilt from the message headers when the index is at
    /// fault, `ActiveMsgs` is recounted with it, a trailing partial lastread
    /// record is dropped and lastread pointers outside the message number
    /// range are clamped. Message text is never rewritten, so anything
    /// reported in [`RepairReport::after`] needs a decision that this library
    /// cannot make.
    pub fn repair(&mut self) -> crate::Result<RepairReport> {
        self.transaction(|base| {
            let before = base.verify_locked()?;
            let reindexed = before.needs_reindex();
            if reindexed {
                base.reindex_locked()?;
            }
            let last_read_bytes_dropped = base.truncate_last_read()?;
            let last_read_records_clamped = base.clamp_last_read()?;
            let after = base.verify_locked()?;
            Ok(RepairReport {
                before,
                after,
                reindexed,
                last_read_bytes_dropped,
                last_read_records_clamped,
            })
        })
    }

    fn verify_locked(&self) -> crate::Result<VerifyReport> {
        let mut report = VerifyReport::default();
        let index = fs::read(self.file_name.with_extension(extensions::MESSAGE_INDEX))?;
        if !index.len().is_multiple_of(INDEX_RECORD_SIZE) {
            report.problems.push(Problem::IndexFileTruncated {
                bytes: index.len() as u64,
            });
        }
        report.index_records = (index.len() / INDEX_RECORD_SIZE) as u32;

        let header_path = self.file_name.with_extension(extensions::HEADER_DATA);
        let header_len = super::file_len(&header_path)?;
        let text_len = super::file_len(&self.file_name.with_extension(extensions::TEXT_DATA))?;
        let mut reader = BufReader::new(File::open(&header_path)?);

        let base_number = self.lowest_message_number();
        let mut seen: BTreeMap<u32, u32> = BTreeMap::new();
        let mut referenced_text = 0u64;

        for (record, data) in index.chunks_exact(INDEX_RECORD_SIZE).enumerate() {
            let message_number = base_number.saturating_add(record as u32);
            let crc = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
            let offset = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
            match (crc == EMPTY_SLOT, offset == EMPTY_SLOT) {
                (true, true) => continue,
                (false, true) => {
                    report
                        .problems
                        .push(Problem::IndexRecordHalfEmpty { message_number });
                    continue;
                }
                _ => {}
            }

            if (offset as u64) < JhrHeaderInfo::JHR_HEADER_SIZE || offset as u64 >= header_len {
                report.problems.push(Problem::IndexOffsetOutOfBounds {
                    message_number,
                    offset,
                });
                continue;
            }
            if let Some(other) = seen.insert(offset, message_number) {
                report.problems.push(Problem::DuplicateHeaderOffset {
                    message_number,
                    other,
                    offset,
                });
            }

            reader.seek(SeekFrom::Start(offset as u64))?;
            let header = match JamMessageHeader::read(&mut reader) {
                Ok(header) => header,
                Err(err) => {
                    report.problems.push(Problem::UnreadableHeader {
                        message_number,
                        reason: err.to_string(),
                    });
                    continue;
                }
            };
            if header.message_number != message_number {
                report.problems.push(Problem::MessageNumberMismatch {
                    message_number,
                    found: header.message_number,
                });
            }
            if header.offset as u64 + header.txt_len as u64 > text_len {
                report.problems.push(Problem::TextOutOfBounds {
                    message_number,
                    offset: header.offset,
                    length: header.txt_len,
                });
            } else {
                referenced_text += header.txt_len as u64;
            }
            if !header.is_deleted() {
                report.live_messages += 1;
            }
        }

        if self.header_info.active_msgs != report.live_messages {
            report.problems.push(Problem::ActiveMessageCountWrong {
                stored: self.header_info.active_msgs,
                counted: report.live_messages,
            });
        }
        let unreferenced = text_len.saturating_sub(referenced_text);
        if unreferenced > 0 {
            report.problems.push(Problem::UnreferencedText {
                bytes: unreferenced,
            });
        }

        self.verify_last_read(&mut report)?;
        Ok(report)
    }

    fn verify_last_read(&self, report: &mut VerifyReport) -> crate::Result<()> {
        let path = self.file_name.with_extension(extensions::LASTREAD_INFO);
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err.into()),
        };
        if !data.len().is_multiple_of(LASTREAD_RECORD_SIZE) {
            report.problems.push(Problem::LastReadFileTruncated {
                bytes: data.len() as u64,
            });
        }
        let highest = self.highest_message_number();
        for record in data.chunks_exact(LASTREAD_RECORD_SIZE) {
            if record[..8] == LASTREAD_DELETED {
                continue;
            }
            let value = |range: std::ops::Range<usize>| {
                u32::from_le_bytes(record[range].try_into().unwrap_or_default())
            };
            let last_read_msg = value(8..12);
            let high_read_msg = value(12..16);
            if last_read_msg > highest || high_read_msg > highest {
                report.problems.push(Problem::LastReadOutOfRange {
                    user_crc: value(0..4),
                    user_id: value(4..8),
                    last_read_msg,
                    high_read_msg,
                });
            }
        }
        Ok(())
    }

    /// Drops a trailing partial lastread record so the file holds whole records.
    fn truncate_last_read(&mut self) -> crate::Result<u64> {
        let path = self.file_name.with_extension(extensions::LASTREAD_INFO);
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(err) => return Err(err.into()),
        };
        let extra = data.len() % LASTREAD_RECORD_SIZE;
        if extra == 0 {
            return Ok(0);
        }
        self.store_last_read_records(&data[..data.len() - extra])?;
        Ok(extra as u64)
    }

    /// Pulls lastread pointers back to the highest message the base addresses.
    fn clamp_last_read(&mut self) -> crate::Result<u32> {
        let mut records = self.read_last_read_file()?;
        let highest = self.highest_message_number();
        let mut clamped = 0;
        for record in &mut records {
            if record.user_crc == u32::MAX && record.user_id == u32::MAX {
                continue;
            }
            if record.last_read_msg > highest || record.high_read_msg > highest {
                record.last_read_msg = record.last_read_msg.min(highest);
                record.high_read_msg = record.high_read_msg.min(highest);
                clamped += 1;
            }
        }
        if clamped > 0 {
            let mut data = Vec::with_capacity(records.len() * LASTREAD_RECORD_SIZE);
            for record in &records {
                record.write(&mut data)?;
            }
            self.store_last_read_records(&data)?;
        }
        Ok(clamped)
    }
}
