use std::{
    fs::{self, File},
    io::{BufReader, Seek},
    path::{Path, PathBuf},
};

use bstr::BString;
use thiserror::Error;

use crate::{Error, util::basic_real::BasicReal};

use self::{
    control::{Conference, ControlDat},
    qwk_message::QwkMessage,
};

pub mod control;
pub mod qwk_message;

#[cfg(test)]
mod tests;

#[derive(Error, Debug)]
pub enum QwkError {
    #[error("Invalid conference number ({0})")]
    CantParseConferenceNumber(BString),

    #[error("Invalid message number ({0})")]
    CantParseMessageNumbers(BString),

    #[error("Invalid message block number ({0})")]
    CantParseMessageBlockNumber(BString),

    #[error("Message number in mail header invalid.")]
    InvalidMessageNumber,
}

pub struct QwkMessageBase {
    path: PathBuf,
    control_dat: ControlDat,
    is_extended: bool,

    pub index_offset_bug: bool,
}

impl QwkMessageBase {
    pub fn bbs_name(&self) -> &BString {
        &self.control_dat.bbs_name
    }

    pub fn bbs_city_and_state(&self) -> &BString {
        &self.control_dat.bbs_city_and_state
    }

    pub fn bbs_phone_number(&self) -> &BString {
        &self.control_dat.bbs_phone_number
    }

    pub fn bbs_sysop_name(&self) -> &BString {
        &self.control_dat.bbs_sysop_name
    }

    pub fn bbs_id(&self) -> &BString {
        &self.control_dat.bbs_id
    }

    pub fn creation_time(&self) -> &BString {
        &self.control_dat.creation_time
    }

    pub fn qmail_user_name(&self) -> &BString {
        &self.control_dat.qmail_user_name
    }

    pub fn qmail_menu_name(&self) -> &BString {
        &self.control_dat.qmail_menu_name
    }

    pub fn message_count(&self) -> u32 {
        self.control_dat.message_count
    }

    pub fn welcome_screen(&self) -> &BString {
        &self.control_dat.welcome_screen
    }

    pub fn news_screen(&self) -> &BString {
        &self.control_dat.news_screen
    }

    pub fn logoff_screen(&self) -> &BString {
        &self.control_dat.logoff_screen
    }

    /// opens an existing message base with base path (without any extension)
    /// extended flag for setting if it's a qwke base
    /// should be safe to always have this enabled.
    pub fn open<P: AsRef<Path>>(path: P, is_extended: bool) -> crate::Result<Self> {
        let control_dat_path = fs::read(path.as_ref().join("control.dat"))?;
        let control_dat = ControlDat::read(&control_dat_path)?;
        Ok(Self {
            path: path.as_ref().to_path_buf(),
            is_extended,
            control_dat,
            index_offset_bug: false,
        })
    }

    pub fn conferences(&self) -> &[Conference] {
        &self.control_dat.conferences
    }

    pub fn read_qwk_index<P: AsRef<Path>>(path: P) -> crate::Result<Vec<u32>> {
        let data = fs::read(path)?;
        Self::convert_qwk_index(data.as_slice())
    }

    /// Turns a .NDX file into message record numbers.
    ///
    /// Each record is a four byte BASIC real followed by the conference number,
    /// which is redundant with the file name and therefore skipped.
    pub fn convert_qwk_index(data: &[u8]) -> crate::Result<Vec<u32>> {
        const NDX_RECORD_SIZE: usize = 5;
        if !data.len().is_multiple_of(NDX_RECORD_SIZE) {
            return Err(Error::qwk(
                0,
                format!(
                    "index size {} is not a multiple of {NDX_RECORD_SIZE}",
                    data.len()
                ),
            ));
        }
        Ok(data
            .chunks_exact(NDX_RECORD_SIZE)
            .map(|record| BasicReal::from([record[0], record[1], record[2], record[3]]).into())
            .collect())
    }

    pub fn read_conference_mail(&self, conference: u16) -> crate::Result<Vec<QwkMessage>> {
        let file_name = format!("{:03}.ndx", conference);
        let index = Self::read_qwk_index(self.path.join(file_name))?;
        let mut res = Vec::with_capacity(index.len());

        let msg_file_name = self.path.join("messages.dat");
        let mut reader = BufReader::new(File::open(msg_file_name)?);
        for block in index {
            // The first block holds the packet header, so record 0 is the second
            // block in the file. Not every writer got that right, hence the flag.
            let block = if self.index_offset_bug {
                block
            } else {
                block.checked_sub(1).ok_or_else(|| {
                    Error::qwk(0, "index record points in front of the first message")
                })?
            };

            reader.seek(std::io::SeekFrom::Start(
                block as u64 * QwkMessage::HEADER_SIZE as u64,
            ))?;
            let mail = QwkMessage::read(&mut reader, self.is_extended)?;
            res.push(mail);
        }

        Ok(res)
    }

    /// Iterates every message in `messages.dat`, skipping the packet header block.
    pub fn iter(&self) -> crate::Result<impl Iterator<Item = crate::Result<QwkMessage>> + use<>> {
        let idx_file_name = self.path.join("messages.dat");
        let mut f = File::open(idx_file_name)?;
        let size = f.metadata()?.len();
        f.seek(std::io::SeekFrom::Start(QwkMessage::HEADER_SIZE as u64))?;
        Ok(QwkMessageIter {
            reader: BufReader::new(f),
            size,
            is_extended: self.is_extended,
        })
    }
}

struct QwkMessageIter {
    reader: BufReader<File>,
    size: u64,
    is_extended: bool,
}

impl Iterator for QwkMessageIter {
    type Item = crate::Result<QwkMessage>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.reader.stream_position() {
            Ok(pos) if pos >= self.size => None,
            Ok(_) => Some(QwkMessage::read(&mut self.reader, self.is_extended)),
            Err(err) => Some(Err(err.into())),
        }
    }
}
