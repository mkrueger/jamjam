use super::convert_str;
use crate::{convert_to_string, convert_u32, convert_u8, util::basic_real::basicreal_to_u32};
use std::{
    fs::File,
    io::{BufReader, Read},
    str::FromStr,
};

#[derive(Clone, Debug)]
pub struct MessageHeader {
    pub status: u8,
    pub msg_number: u32,
    pub ref_number: u32,
    pub num_blocks: u8,
    pub date: String,
    pub time: String,
    pub to_field: String,
    pub reply_date: u32,
    pub reply_time: String,
    pub reply_status: u8,
    pub from_field: String,
    pub subj_field: String,
    pub password: String,
    pub active_flag: u8,
    pub echo_flag: u8,
    pub reserved: u32,
    pub extended_status: u8,
    pub net_tag: u8,
}

impl MessageHeader {
    pub const HEADER_SIZE: usize =
        1 + 4 + 4 + 1 + 8 + 5 + 25 + 4 + 5 + 1 + 25 + 25 + 8 + 1 + 1 + 4 + 1 + 1;

    pub fn read(file: &mut BufReader<File>) -> crate::Result<Self> {
        let data = &mut [0; Self::HEADER_SIZE];
        file.read_exact(data)?;
        let mut data = &data[..];

        convert_u8!(status, data);
        convert_u32!(msg_number, data);
        let msg_number = basicreal_to_u32(msg_number);
        convert_u32!(ref_number, data);
        convert_u8!(num_blocks, data);
        convert_to_string!(date, data, 8);
        convert_to_string!(time, data, 5);
        convert_to_string!(to_field, data, 25);
        convert_u32!(reply_date, data);
        convert_to_string!(reply_time, data, 5);
        convert_u8!(reply_status, data);
        convert_to_string!(from_field, data, 25);
        convert_to_string!(subj_field, data, 25);
        convert_to_string!(password, data, 8);
        convert_u8!(active_flag, data);
        convert_u8!(echo_flag, data);
        convert_u32!(reserved, data);
        convert_u8!(extended_status, data);
        convert_u8!(net_tag, data);

        Ok(Self {
            status,
            msg_number,
            ref_number,
            num_blocks,
            date,
            time,
            to_field,
            reply_date,
            reply_time,
            reply_status,
            from_field,
            subj_field,
            password,
            active_flag,
            echo_flag,
            reserved,
            extended_status,
            net_tag,
        })
    }
}

#[derive(Debug)]
pub enum ExtendedHeaderInformation {
    To,
    From,
    Subject,
    Attach,
    List,
    Route,
    Origin,
    Reqrr,
    Ackrr,
    Ackname,
    Packout,
    To2,
    From2,
    Forward,
    Ufollow,
    Unewsgr,
}

impl FromStr for ExtendedHeaderInformation {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "TO" => Ok(Self::To),
            "FROM" => Ok(Self::From),
            "SUBJECT" => Ok(Self::Subject),
            "ATTACH" => Ok(Self::Attach),
            "LIST" => Ok(Self::List),
            "ROUTE" => Ok(Self::Route),
            "ORIGIN" => Ok(Self::Origin),
            "REQRR" => Ok(Self::Reqrr),
            "ACKRR" => Ok(Self::Ackrr),
            "ACKNAME" => Ok(Self::Ackname),
            "PACKOUT" => Ok(Self::Packout),
            "TO2" => Ok(Self::To2),
            "FROM2" => Ok(Self::From2),
            "FORWARD" => Ok(Self::Forward),
            "UFOLLOW" => Ok(Self::Ufollow),
            "UNEWSGR" => Ok(Self::Unewsgr),
            _ => Err(()),
        }
    }
}

impl ExtendedHeaderInformation {
    pub fn to_str(&self) -> &'static str {
        match self {
            Self::To => "TO     ",
            Self::From => "FROM   ",
            Self::Subject => "SUBJECT",
            Self::Attach => "ATTACH ",
            Self::List => "LIST   ",
            Self::Route => "ROUTE  ",
            Self::Origin => "ORIGIN ",
            Self::Reqrr => "REQRR  ",
            Self::Ackrr => "ACKRR  ",
            Self::Ackname => "ACKNAME",
            Self::Packout => "PACKOUT",
            Self::To2 => "TO2    ",
            Self::From2 => "FROM2  ",
            Self::Forward => "FORWARD",
            Self::Ufollow => "UFOLLOW",
            Self::Unewsgr => "UNEWSGR",
        }
    }
}

pub struct ExtendedHeader {
    pub info: ExtendedHeaderInformation,
    pub content: String,
    pub status: u8,
}

impl ExtendedHeader {
    // const ID:u16 = 0x40FF;
    const FUNC_LEN: usize = 7;
    const DESC_LEN: usize = 60;

    pub fn deserialize(buf: &[u8]) -> crate::Result<Self> {
        // let _id = u16::from_le_bytes([buf[0], buf[1]]);
        let mut i = 2;
        let function =
            ExtendedHeaderInformation::from_str(&convert_str(&buf[i..i + Self::FUNC_LEN])).unwrap();
        i += Self::FUNC_LEN + 1; // skip ':'

        let content = convert_str(&buf[i..i + Self::DESC_LEN]);
        i += Self::DESC_LEN;

        let status = buf[i];
        Ok(Self {
            info: function,
            content,
            status,
        })
    }
}
