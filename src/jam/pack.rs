use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};

use crate::util::crc32::CRC_SEED;

use super::{
    INDEX_RECORD_SIZE, JAM_SIGNATURE, JamError, JamMessageBase, extensions,
    jhr_header::JhrHeaderInfo, msg_header::JamMessageHeader, msg_header::SubfieldType,
};

/// "If both ulongs are -1 (ffffffffH), there is no corresponding message header."
pub(crate) const EMPTY_SLOT: u32 = 0xFFFF_FFFF;

/// The criteria PCBPack was driven with, which is what the sysop is asked for
/// when packing a conference from the board.
#[derive(Default, Clone)]
pub struct PackOptions {
    /// Throw the index away and rebuild it from the headers (`/INDEX`). No
    /// message is removed and the header and text files are left alone.
    pub index_only: bool,

    /// Remove messages written before this date (`/DATE`).
    pub purge_before: Option<DateTime<Utc>>,

    /// Remove private messages the recipient has already read (`/PURGE`).
    pub purge_received_private: bool,

    /// Renumber the kept messages starting at this number (`/RENUMBER`).
    /// Like the original, last-read pointers are not adjusted.
    pub renumber_from: Option<u32>,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct PackReport {
    /// Messages the base held before packing.
    pub before: u32,
    pub removed: u32,
    pub kept: u32,
    pub low_message_number: u32,
    pub high_message_number: u32,
}

/// A slot of the message index, either pointing at a header or empty.
type Slot = Option<(u32, u32)>;

impl JamMessageBase {
    /// Removes messages and reclaims the space they held in the header and text
    /// files - PCBoard ran PCBPack for this.
    ///
    /// Unless the base is renumbered the message numbers of the surviving
    /// messages do not change, so last-read pointers stay valid.
    pub fn pack(&mut self, options: &PackOptions) -> crate::Result<PackReport> {
        self.transaction(|base| base.pack_locked(options))
    }

    fn pack_locked(&mut self, options: &PackOptions) -> crate::Result<PackReport> {
        if options.index_only {
            return self.reindex_locked();
        }
        let now = Utc::now();

        let slots = self.read_index()?;
        let header_path = self.file_path(extensions::HEADER_DATA);
        let text_path = self.file_path(extensions::TEXT_DATA);

        let mut header_reader = BufReader::new(File::open(&header_path)?);
        let mut text_reader = BufReader::new(File::open(&text_path)?);
        let text_file_len = fs::metadata(&text_path)?.len();

        let tmp_header_path = temp_path(&header_path);
        let tmp_text_path = temp_path(&text_path);
        let tmp_index_path = temp_path(&self.file_path(extensions::MESSAGE_INDEX));

        let mut report = PackReport::default();
        let mut new_slots: Vec<Slot> = Vec::with_capacity(slots.len());
        let mut header_offset = JhrHeaderInfo::JHR_HEADER_SIZE;
        let mut text_offset = 0u64;

        {
            let mut header_writer = BufWriter::new(File::create(&tmp_header_path)?);
            let mut text_writer = BufWriter::new(File::create(&tmp_text_path)?);
            header_writer.write_all(&[0; JhrHeaderInfo::JHR_HEADER_SIZE as usize])?;

            for slot in &slots {
                let Some((_, offset)) = slot else {
                    new_slots.push(None);
                    continue;
                };
                report.before += 1;

                header_reader.seek(SeekFrom::Start(*offset as u64))?;
                let mut header = JamMessageHeader::read(&mut header_reader).map_err(|err| {
                    crate::Error::jam(*offset as u64, format!("unreadable indexed header: {err}"))
                })?;

                if should_remove(&header, options, now) {
                    new_slots.push(None);
                    report.removed += 1;
                    continue;
                }

                let text_end = header.offset as u64 + header.txt_len as u64;
                if text_end > text_file_len {
                    return Err(JamError::TextOutOfBounds(
                        header.offset as u64,
                        text_end - text_file_len,
                    )
                    .into());
                }
                let mut text = vec![0; header.txt_len as usize];
                text_reader.seek(SeekFrom::Start(header.offset as u64))?;
                text_reader.read_exact(&mut text)?;

                header.offset = super::offset_u32(text_offset)?;
                header.txt_len = text.len() as u32;
                text_writer.write_all(&text)?;
                text_offset += text.len() as u64;

                if let Some(first) = options.renumber_from {
                    header.message_number = first + report.kept;
                }

                let crc = header.to().map_or(CRC_SEED, JamMessageBase::crc);
                header.write(&mut header_writer)?;
                new_slots.push(Some((crc, super::offset_u32(header_offset)?)));
                header_offset += header_size(&header) as u64;
                report.kept += 1;
            }

            text_writer.flush()?;
            header_writer.flush()?;
        }

        let (base_msg_num, index) = if let Some(first) = options.renumber_from {
            (first, new_slots.into_iter().flatten().map(Some).collect())
        } else {
            (self.header_info.base_msg_num, new_slots)
        };

        fs::write(&tmp_index_path, index_bytes(&index))?;
        self.header_info.base_msg_num = base_msg_num;
        self.header_info.active_msgs = report.kept;
        self.index_records = index.len() as u32;
        write_base_header(&tmp_header_path, &mut self.header_info)?;

        // The three files only make sense together, but they can only be moved
        // into place one at a time. Get them onto the disk first, then leave a
        // marker so that an interrupted run can be finished on the next open.
        sync_file(&tmp_text_path)?;
        sync_file(&tmp_header_path)?;
        sync_file(&tmp_index_path)?;

        let marker = pack_marker(&self.file_name);
        fs::write(&marker, PACK_MARKER_CONTENT)?;
        sync_file(&marker)?;
        sync_dir(&self.file_name);

        fs::rename(&tmp_text_path, &text_path)?;
        fs::rename(&tmp_header_path, &header_path)?;
        fs::rename(&tmp_index_path, self.file_path(extensions::MESSAGE_INDEX))?;
        sync_dir(&self.file_name);
        fs::remove_file(&marker)?;
        sync_dir(&self.file_name);

        report.low_message_number = self.lowest_message_number();
        report.high_message_number = self.highest_message_number();
        Ok(report)
    }

    /// Rebuilds the index from the message headers, PCBPack's `/INDEX`. Useful
    /// when the index is suspected to be out of step with the headers.
    ///
    /// Deleted messages keep their slot so they can still be recovered.
    pub fn reindex(&mut self) -> crate::Result<PackReport> {
        self.transaction(|base| base.reindex_locked())
    }

    fn reindex_locked(&mut self) -> crate::Result<PackReport> {
        let header_path = self.file_path(extensions::HEADER_DATA);
        let file = File::open(&header_path)?;
        let size = file.metadata()?.len();
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(JhrHeaderInfo::JHR_HEADER_SIZE))?;

        // A header that grew was appended instead of overwritten, so the last
        // copy of a message number is the current one.
        let mut found: BTreeMap<u32, (u32, u32, bool)> = BTreeMap::new();
        let mut offset = JhrHeaderInfo::JHR_HEADER_SIZE;
        while offset < size {
            let header = JamMessageHeader::read(&mut reader).map_err(|err| {
                crate::Error::jam(offset, format!("unreadable header during reindex: {err}"))
            })?;
            if header.message_number >= self.header_info.base_msg_num {
                let crc = header.to().map_or(CRC_SEED, JamMessageBase::crc);
                found.insert(
                    header.message_number,
                    (crc, super::offset_u32(offset)?, header.is_deleted()),
                );
            }
            offset += header_size(&header) as u64;
        }

        let base = self.header_info.base_msg_num;
        let high = found
            .keys()
            .next_back()
            .copied()
            .unwrap_or(base.saturating_sub(1));
        let mut index: Vec<Slot> = vec![None; (high + 1 - base) as usize];
        for (number, (crc, offset, _)) in &found {
            index[(number - base) as usize] = Some((*crc, *offset));
        }

        let tmp_index_path = temp_path(&self.file_path(extensions::MESSAGE_INDEX));
        fs::write(&tmp_index_path, index_bytes(&index))?;
        sync_file(&tmp_index_path)?;
        fs::rename(&tmp_index_path, self.file_path(extensions::MESSAGE_INDEX))?;

        let active = found.values().filter(|(_, _, deleted)| !deleted).count() as u32;
        self.header_info.active_msgs = active;
        self.index_records = index.len() as u32;
        self.write_jhr_header()?;

        Ok(PackReport {
            before: found.len() as u32,
            removed: 0,
            kept: found.len() as u32,
            low_message_number: base,
            high_message_number: high,
        })
    }

    fn file_path(&self, extension: &str) -> PathBuf {
        self.file_name.with_extension(extension)
    }

    /// The index as slots, `None` where the record marks that no header exists.
    fn read_index(&self) -> crate::Result<Vec<Slot>> {
        let data = fs::read(self.file_path(extensions::MESSAGE_INDEX))?;
        if !data.len().is_multiple_of(INDEX_RECORD_SIZE) {
            return Err(JamError::IndexFileCorrupted.into());
        }
        data.chunks_exact(INDEX_RECORD_SIZE)
            .enumerate()
            .map(|(record_number, record)| {
                let crc = u32::from_le_bytes([record[0], record[1], record[2], record[3]]);
                let offset = u32::from_le_bytes([record[4], record[5], record[6], record[7]]);
                match (crc == EMPTY_SLOT, offset == EMPTY_SLOT) {
                    (true, true) => Ok(None),
                    (false, true) => Err(JamError::InvalidIndexRecord(record_number as u64).into()),
                    (_, false) => Ok(Some((crc, offset))),
                }
            })
            .collect()
    }
}

fn header_size(header: &JamMessageHeader) -> usize {
    JamMessageHeader::FIXED_HEADER_SIZE
        + header
            .sub_fields
            .iter()
            .map(|field| 8 + field.content().len())
            .sum::<usize>()
}

fn index_bytes(slots: &[Slot]) -> Vec<u8> {
    let mut data = Vec::with_capacity(slots.len() * INDEX_RECORD_SIZE);
    for slot in slots {
        let (crc, offset) = slot.unwrap_or((EMPTY_SLOT, EMPTY_SLOT));
        data.extend(crc.to_le_bytes());
        data.extend(offset.to_le_bytes());
    }
    data
}

/// Written while the packed files are moved into place, so an interrupted run
/// can be spotted and finished.
const PACK_MARKER_CONTENT: &[u8] = b"jamjam pack in progress\n";

fn pack_marker(file_name: &Path) -> PathBuf {
    file_name.with_extension("jampack")
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".packing");
    PathBuf::from(name)
}

fn sync_file(path: &Path) -> crate::Result<()> {
    OpenOptions::new()
        .write(true)
        .truncate(false)
        .open(path)?
        .sync_all()?;
    Ok(())
}

/// Renames are only durable once the directory entry is on the disk. Platforms
/// that do not allow opening a directory simply skip this.
pub(crate) fn sync_dir(file_name: &Path) {
    if let Some(parent) = file_name.parent().filter(|p| !p.as_os_str().is_empty())
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }
}

/// Finishes a pack that was cut short before all three files were in place.
///
/// The temporaries are complete and consistent by the time the marker appears,
/// so whichever ones have not been moved yet can simply be moved now.
pub(crate) fn recover_interrupted_pack(file_name: &Path) -> crate::Result<()> {
    let marker = pack_marker(file_name);
    if !marker.exists() {
        return Ok(());
    }
    log::warn!("Completing an interrupted pack of {}", file_name.display());
    for extension in [
        extensions::TEXT_DATA,
        extensions::HEADER_DATA,
        extensions::MESSAGE_INDEX,
    ] {
        let target = file_name.with_extension(extension);
        let tmp = temp_path(&target);
        if tmp.exists() {
            fs::rename(&tmp, &target)?;
        }
    }
    sync_dir(file_name);
    fs::remove_file(&marker)?;
    sync_dir(file_name);
    Ok(())
}

/// Writes the 1024 byte record the packed header file starts with.
fn write_base_header(path: &Path, info: &mut JhrHeaderInfo) -> crate::Result<()> {
    let mut file = OpenOptions::new().write(true).truncate(false).open(path)?;
    file.write_all(&JAM_SIGNATURE)?;
    file.write_all(&info.date_created.to_le_bytes())?;
    let mut writer = BufWriter::new(file);
    info.update(&mut writer)?;
    writer.flush()?;
    Ok(())
}

fn should_remove(header: &JamMessageHeader, options: &PackOptions, now: DateTime<Utc>) -> bool {
    if header.is_deleted() {
        return true;
    }
    if let Some(packout) = packout_date(header)
        && packout <= now
    {
        return true;
    }
    if let Some(cutoff) = options.purge_before
        && message_date(header).is_some_and(|date| date < cutoff)
    {
        return true;
    }
    options.purge_received_private && header.is_private() && header.is_read()
}

fn packout_date(header: &JamMessageHeader) -> Option<DateTime<Utc>> {
    let field = header
        .sub_fields
        .iter()
        .find(|field| field.field_type() == SubfieldType::PackoutDate)?;
    let text = String::from_utf8_lossy(field.content());
    DateTime::parse_from_rfc3339(&text)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

fn message_date(header: &JamMessageHeader) -> Option<DateTime<Utc>> {
    let seconds = if header.date_written != 0 {
        header.date_written
    } else {
        header.date_processed
    };
    if seconds == 0 {
        return None;
    }
    DateTime::from_timestamp(seconds as i64, 0)
}
