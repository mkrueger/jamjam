use std::fs;
use std::io::{BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::{error::Error, fs::File, io::Read};

use rayon::iter::{IntoParallelIterator, ParallelIterator};
use thiserror::Error;
pub mod crc32;
mod macros;

#[derive(Error, Debug)]
pub enum JamError {
    #[error("Invalid header")]
    InvalidHeader,

    #[error("Index file corrupted")]
    IndexFileCorrupted,

    #[error("Unsupported message header revision: {0}")]
    UnsupportedMessageHeaderRevision(u16),
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

#[derive(Debug, Default)]
pub struct HeaderInfo {
    /// <J><A><M> followed by <NUL>
    //pub signature: u32,

    /// Creation date
    pub datecreated: u32,
    /// Update counter
    pub modcounter: u32,
    /// Number of active (not deleted) msgs  
    pub activemsgs: u32,
    /// CRC-32 of password to access
    pub passwordcrc: u32,
    /// Lowest message number in index file
    pub basemsgnum: u32,
    // Reserved space (currently unused)
    // pub reserved: [u8; 1000]
}

const JAM_SIGNATURE: [u8; 4] = [b'J', b'A', b'M', 0];

impl HeaderInfo {
    const HEADER_INFO_SIZE: usize = 24;

    pub fn load(file: &mut File) -> Result<Self, Box<dyn Error>> {
        let data = &mut [0; Self::HEADER_INFO_SIZE];
        file.read_exact(data)?;
        if !data.starts_with(&JAM_SIGNATURE) {
            return Err(Box::new(JamError::InvalidHeader));
        }
        let mut data = &data[4..];
        convert_u32!(datecreated, data);
        convert_u32!(modcounter, data);
        convert_u32!(activemsgs, data);
        convert_u32!(passwordcrc, data);
        convert_u32!(basemsgnum, data);
        Ok(Self {
            datecreated,
            modcounter,
            activemsgs,
            passwordcrc,
            basemsgnum,
        })
    }

    fn _create<P: AsRef<Path>>(&self, file_name: &P) -> Result<(), Box<dyn Error>> {
        let mut result = JAM_SIGNATURE.to_vec();
        result.extend(&self.datecreated.to_le_bytes());
        result.extend(&self.modcounter.to_le_bytes());
        result.extend(&self.activemsgs.to_le_bytes());
        result.extend(&self.passwordcrc.to_le_bytes());
        result.extend(&self.basemsgnum.to_le_bytes());
        result.extend(&[0; 1000]); // Reserved space (currently unused)
        fs::write(file_name, result)?;
        Ok(())
    }

    fn _update_activemsgs(&mut self, file: &mut File) -> Result<(), Box<dyn Error>> {
        file.seek(std::io::SeekFrom::Start(8))?;
        self.modcounter = self.modcounter.wrapping_add(1);
        file.write_all(&self.modcounter.to_le_bytes())?;
        file.write_all(&self.activemsgs.to_le_bytes())?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct MessageHeader {
    /// <J><A><M> followed by <NUL>
    //pub signature: u32,

    /// Revision level of header
    ///
    /// # Remarks
    /// This field is intended for future revisions of the specifications
    /// to allow the use of a different fixed-length binary message
    /// header. The current revision level is one (1).
    //
    // pub revision: u16,

    /// Reserved for future use
    // unused
    //    pub reserved_word: u16,

    /// Length of subfields
    ///
    /// The SubfieldLen field is set to zero (0) if the header does not
    /// have any subfield data. I.e. the length of the binary header is
    /// not included in this field.
    pub sub_fields: Vec<MessageSubfield>,

    // unused
    // pub subfield_len: u32,
    /// Number of times message read
    pub times_read: u32,

    /// CRC-32 of MSGID line
    ///
    /// # Remarks
    /// When calculating the CRC-32 of the MSGID and REPLY lines, the
    /// text ^aMSGID: and ^aREPLY: should be removed as well as all
    /// leading and trailing white space characters.
    pub msgid_crc: u32,

    /// CRC-32 of REPLY line
    ///
    /// # Remarks
    /// When calculating the CRC-32 of the MSGID and REPLY lines, the
    /// text ^aMSGID: and ^aREPLY: should be removed as well as all
    /// leading and trailing white space characters.
    pub replycrc: u32,

    /// This msg is a reply to..
    pub reply_to: u32,

    /// First reply to this msg
    pub reply1st: u32,

    /// Next msg in reply chain
    pub replynext: u32,

    /// When msg was written
    pub date_written: u32,

    /// When msg was read by recipient
    pub date_received: u32,

    /// When msg was processed by tosser/scanner
    pub date_processed: u32,

    /// Message number (1-based)
    pub message_number: u32,

    /// Msg attribute, see "Msg Attributes"
    pub attribute: u32,

    /// Reserved for future use
    pub attribute2: u32,

    /// Offset of text in ????????.JDT file
    pub offset: u32,

    /// Length of message text
    pub txt_len: u32,

    /// CRC-32 of password to access message
    pub password_crc: u32,

    /// Cost of message
    pub cost: u32,
}

impl MessageHeader {
    const FIXED_HEADER_SIZE: usize = 76;

    pub fn get_subject(&self) -> Option<String> {
        for s in &self.sub_fields {
            if s.get_type() == SubfieldType::Subject {
                return Some(s.get_string());
            }
        }
        None
    }

    pub fn get_from(&self) -> Option<String> {
        for s in &self.sub_fields {
            if s.get_type() == SubfieldType::SenderName {
                return Some(s.get_string());
            }
        }
        None
    }

    pub fn get_to(&self) -> Option<String> {
        for s in &self.sub_fields {
            if s.get_type() == SubfieldType::RecvName {
                return Some(s.get_string());
            }
        }
        None
    }

    pub fn read(file: &mut BufReader<File>) -> Result<Self, Box<dyn Error>> {
        let data = &mut [0; Self::FIXED_HEADER_SIZE];
        file.read_exact(data)?;
        if !data.starts_with(&JAM_SIGNATURE) {
            return Err(Box::new(JamError::InvalidHeader));
        }
        let data = &data[4..];
        convert_single_u16!(revision, data);
        if revision != 1 {
            return Err(Box::new(JamError::UnsupportedMessageHeaderRevision(
                revision,
            )));
        }
        let mut data = &data[4..];
        // convert_u32!(reserved_word, data);
        convert_u32!(subfield_len, data);

        convert_u32!(times_read, data);
        convert_u32!(msgid_crc, data);
        convert_u32!(reply_crc, data);
        convert_u32!(reply_to, data);
        convert_u32!(reply1st, data);
        convert_u32!(replynext, data);
        convert_u32!(date_written, data);
        convert_u32!(date_received, data);
        convert_u32!(date_processed, data);
        convert_u32!(message_number, data);
        convert_u32!(attribute, data);
        convert_u32!(attribute2, data);
        convert_u32!(offset, data);
        convert_u32!(txt_len, data);
        convert_u32!(password_crc, data);
        convert_u32!(cost, data);

        let mut subfield_data = vec![0; subfield_len as usize];
        file.read_exact(&mut subfield_data)?;

        let mut sub_fields = Vec::new();

        let mut i = 0;
        while i < subfield_len as usize {
            sub_fields.push(MessageSubfield::deserialize(&subfield_data, &mut i)?);
        }
        Ok(Self {
            sub_fields,
            times_read,
            msgid_crc,
            replycrc: reply_crc,
            reply_to,
            reply1st,
            replynext,
            date_written,
            date_received,
            date_processed,
            message_number,
            attribute,
            attribute2,
            offset,
            txt_len,
            password_crc,
            cost,
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum SubfieldType {
    /// Unknown subfield type
    Unknown = 0xFFFF,

    /// A network address. This is used to specify the originating address.
    /// More than one OADDRESS field may exist. DATLEN must not exceed 100
    /// characters. For a FidoNet-style address, this field must follow the
    /// ZONE:NET/NODE.POINT@DOMAIN format where .POINT is excluded if zero
    /// and @DOMAIN is excluded if unknown.
    Address0 = 0,

    /// network address. This is used to specify the destination address.
    /// More than one DADDRESS field may exist (e.g. carbon copies). DATLEN
    /// must not exceed 100 characters. For a FidoNet-style address, this
    /// field must follow the ZONE:NET/NODE.POINT@DOMAIN format where .POINT
    /// is excluded if zero and @DOMAIN is excluded if unknown.
    AddressD = 1,

    /// The sender (author) of the message. DATLEN must not exceed 100 characters.
    SenderName = 2,

    /// The recipient of the message. DATLEN must not exceed 100 characters.
    RecvName = 3,

    /// Used to store the message identification data. All data not relevant
    /// to the actual ID string, including leading and trailing white space
    /// characters should be removed. DATLEN must not exceed 100 characters.
    MsgID = 4,

    /// Used to store the message reply data. All data not relevant to the
    /// actual reply string, including leading and trailing white space
    /// characters should be removed. DATLEN must not exceed 100 characters.
    ReplyID = 5,

    /// The subject of the message. DATLEN must not exceed 100 characters.
    /// Note that this field may not be used for FidoNet-style file attaches
    /// or file requests.
    Subject = 6,

    /// Used to store the FTN PID kludge line. Only the actual PID data is
    /// stored and ^aPID: is stripped along with any leading and trailing
    /// white space characters. DATLEN must not exceed 40 characters.
    PID = 7,

    /// This is also referred to as ^aVia information in FTNs. It contains
    /// information about a system which the message has travelled through.
    /// The format of the field is  where:
    ///
    /// YYYY is the year (1992-9999)
    ///   MM is the month (01-12)
    ///   DD is the day (01-31)
    ///   HH is the hour (00-23)
    ///   MM is the minute (00-59)
    ///   SS is the second (00-59)
    ///
    /// The timestamp is stored in ASCII (0-9) characters. The network
    /// address is the address of the system. It is expressed in ASCII
    /// notation in the native format of the forwarding system.
    Trace = 8,

    /// A file attached to the message. Only one filename may be specified
    /// per subfield. No wildcard characters are allowed. If this subfield
    /// is present in a message header, the ATTRIBUTE must include the
    /// MSG_FILEATTACH bit.
    EnclFile = 9,

    /// Identical to ENCLOSEDFILE with the exception that the filename is
    /// followed by a  (00H) and an alias filename to be transmited to
    /// the remote system in place of the local name of the file.
    EnclFwAlias = 10,

    /// A request for one or more files. Only one filemask may be specified
    /// per subfield. If the filemask contains a complete path, it is to be
    /// regarded as an update file request. If this subfield is present in a
    /// message header, the ATTRIBUTE must include the MSG_FILEREQUEST bit.
    /// To indicate that a password is to be transmitted along with the
    /// request, a  (00H) character followed by the password is
    /// appended. E.g. SECRET*.*MYPASSWORD.
    EnclFreq = 11,

    /// One or more files attached to the message. Only one filename may be
    /// specified per subfield. Wildcard characters are allowed. If this
    /// subfield is present in a message header, the ATTRIBUTE must include
    /// the MSG_FILEATTACH bit.
    EnclFieleWc = 12,

    /// One or more files attached to the message. The filename points to an
    /// ASCII file with one filename entry per line. If alias filenames are
    /// to be used, they are specified after the actual filename and
    /// separated by a  (00H) character, e.g. C:\MYFILE.LZHNEWS.
    /// Wildcard characters are not allowed.
    EnclIndFile = 13,

    /// Reserved for future use.
    EmbInDat = 1000,

    /// An FTS-compliant "kludge" line not otherwise represented here. All
    /// data not relevant to the actual kludge line, including leading and
    /// trailing white space and ^A (01H) characters should be removed.
    /// DATLEN must not exceed 255 characters. The FTS kludges INTL, TOPT,
    /// and FMPT must never be stored as separate SubFields. Their data must
    /// be extracted and used for the address SubFields.
    FTSKludge = 2000,

    /// Used to store two-dimensional (net/node) SEEN-BY information often
    /// used in FTN conference environments. Only the actual SEEN-BY data is
    /// stored and ^aSEEN-BY: or SEEN-BY: is stripped along with any leading
    /// and trailing white space characters.
    SeenBy2D = 2001,

    /// Used to store two-dimensional (net/node) PATH information often used
    /// in FTN conference environments. Only the actual PATH data is stored
    /// and ^aPATH: is stripped along with any leading and trailing white
    /// space characters.
    Path2D = 2002,

    /// Used to store the FTN FLAGS kludge information. Note that all FLAG
    /// options that have binary representation in the JAM message header
    /// must be removed from the FLAGS string prior to storing it. Only
    /// the actual flags option string is stored and ^aFLAGS is stripped
    /// along with any leading and trailing white space characters.
    Flags = 2003,

    /// Time zone information. This subfield consists of four mandatory
    /// bytes and one optional. The first character may be a plus (+) or a
    /// minus (-) character to indicate a location east (plus) or west
    /// (minus) of UTC 0000. The plus character is implied unless the first
    /// character is a minus character. The following four bytes must be
    /// digits in the range zero through nine and indicates the offset in
    /// hours and minutes. E.g. 0100 indicates an offset of one hour east of
    /// UTC.
    TZUTCInfo = 2204,
}

#[derive(Debug)]
pub struct MessageSubfield {
    id: u32, // was lo_id/hi_id - but hi_id is unused
    buffer: Vec<u8>,
}

impl MessageSubfield {
    pub fn get_string(&self) -> String {
        String::from_utf8_lossy(&self.buffer).to_string()
    }

    pub fn get_type(&self) -> SubfieldType {
        match self.id {
            0 => SubfieldType::Address0,
            1 => SubfieldType::AddressD,
            2 => SubfieldType::SenderName,
            3 => SubfieldType::RecvName,
            4 => SubfieldType::MsgID,
            5 => SubfieldType::ReplyID,
            6 => SubfieldType::Subject,
            7 => SubfieldType::PID,
            8 => SubfieldType::Trace,
            9 => SubfieldType::EnclFile,
            10 => SubfieldType::EnclFwAlias,
            11 => SubfieldType::EnclFreq,
            12 => SubfieldType::EnclFieleWc,
            13 => SubfieldType::EnclIndFile,
            1000 => SubfieldType::EmbInDat,
            2000 => SubfieldType::FTSKludge,
            2001 => SubfieldType::SeenBy2D,
            2002 => SubfieldType::Path2D,
            2003 => SubfieldType::Flags,
            2204 => SubfieldType::TZUTCInfo,
            _ => SubfieldType::Unknown,
        }
    }

    fn deserialize(sub_fields: &[u8], idx: &mut usize) -> Result<Self, Box<dyn Error>> {
        let mut data = &sub_fields[*idx..];
        convert_u32!(id, data);
        convert_u32!(data_len, data);
        *idx += 8;
        let end = *idx + data_len as usize;
        let buffer = sub_fields[*idx..end].to_vec();
        *idx = end;
        Ok(Self { id, buffer })
    }
}

#[derive(Default, Debug)]
pub struct LastReadStorage {
    pub user_crc: u32,      // CRC-32 of user name (lowercase)   (1)
    pub user_id: u32,       // Unique UserID
    pub last_read_msg: u32, // Last read message number
    pub high_read_msg: u32, // Highest read message number
}

impl LastReadStorage {
    const LAST_READ_SIZE: usize = 16;

    pub fn load(file: &mut BufReader<File>) -> Result<Self, Box<dyn Error>> {
        let data = &mut [0; Self::LAST_READ_SIZE];
        file.read_exact(data)?;
        let mut data = &data[..];
        convert_u32!(user_crc, data);
        convert_u32!(user_id, data);
        convert_u32!(last_read_msg, data);
        convert_u32!(high_read_msg, data);
        Ok(Self {
            user_crc,
            user_id,
            last_read_msg,
            high_read_msg,
        })
    }

    pub fn write(&self, file: &mut File) -> Result<(), Box<dyn Error>> {
        file.write_all(&self.user_crc.to_le_bytes())?;
        file.write_all(&self.user_id.to_le_bytes())?;
        file.write_all(&self.last_read_msg.to_le_bytes())?;
        file.write_all(&self.high_read_msg.to_le_bytes())?;
        Ok(())
    }
}

pub struct MessageBase {
    file_name: PathBuf,

    header_info: HeaderInfo,

    last_read_record: i32,

    is_locked: bool,
}

impl MessageBase {
    const HEADER_START: u64 = 1024;

    pub fn new(file_name: impl Into<PathBuf>) -> Self {
        Self {
            file_name: file_name.into(),
            header_info: HeaderInfo::default(),
            last_read_record: -1,
            is_locked: false,
        }
    }

    pub fn lock(&mut self) -> Result<bool, Box<dyn Error>> {
        if self.is_locked {
            return Ok(false);
        }
        self.is_locked = true;
        Ok(true)
    }

    pub fn unlock(&mut self) -> Result<(), Box<dyn Error>> {
        self.is_locked = false;
        Ok(())
    }

    pub fn read_header_info(&mut self) -> Result<(), Box<dyn Error>> {
        let header_file_name = self.file_name.with_extension(extensions::HEADER_DATA);
        let mut header = File::open(header_file_name)?;
        self.header_info = HeaderInfo::load(&mut header)?;
        Ok(())
    }

    pub fn get_msg_text(&self, header: &MessageHeader) -> Result<String, Box<dyn Error>> {
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

    pub fn read_headers(&self) -> Result<Vec<MessageHeader>, Box<dyn Error>> {
        let header_file_name = self.file_name.with_extension(extensions::HEADER_DATA);
        let header_file = File::open(header_file_name)?;
        let mut reader = BufReader::new(header_file);
        reader.seek(SeekFrom::Start(Self::HEADER_START))?;
        let mut res = Vec::new();
        while let Ok(header) = MessageHeader::read(&mut reader) {
            res.push(header);
        }
        Ok(res)
    }

    pub fn read_last_read_file(&self) -> Result<Vec<LastReadStorage>, Box<dyn Error>> {
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
        crc: u32,
        id: u32,
    ) -> Result<Option<LastReadStorage>, Box<dyn Error>> {
        let last_read_file_name = self.file_name.with_extension(extensions::LASTREAD_INFO);
        let file = File::open(last_read_file_name)?;
        let mut reader = BufReader::new(file);

        let id_bytes = id.to_le_bytes();
        let crc_bytes = crc.to_le_bytes();

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
                    user_crc: crc,
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
    pub fn search_message_index(&self, crc: u32) -> Result<Vec<u32>, Box<dyn Error>> {
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

/*
fn main() {
    let now = Instant::now();

    let msg_base = MessageBase::new("/home/mkrueger/mystic/msgs/general");

    for hdr in msg_base.read_headers().unwrap() {
        println!("Subj: {:?}", hdr.get_subject());
        println!("From: {:?}", hdr.get_from());
        println!("To: {:?}", hdr.get_to());

        let txt = msg_base.get_msg_text(&hdr).unwrap();
        println!("----------- {}", txt.len());
        println!("{}", txt);
        println!("-----------");
    }

    println!("Elapsed time: {:?}", now.elapsed());
}
*/
