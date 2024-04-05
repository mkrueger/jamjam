use std::{fs::File, io::Read};

use crate::{convert_u32, pcboard::convert_str, util::basic_real::basicreal_to_u32};

pub struct MessageBaseHeader {
    /// Highest message number in index file
    pub high_msg_num: u32,
    /// Lowest message number in index file
    pub low_msg_num: u32,
    /// Active (non deleted) messages
    pub active_msgs: u32,

    /// Number of callers for this conference.
    /// Note that PCBoard only allows 1 messege base per conference.
    pub callers: u32,

    pub lock_status: String,
}

impl MessageBaseHeader {
    pub const HEADER_SIZE: usize = 4 * 4 + 6;

    pub fn load(file: &mut File) -> crate::Result<Self> {
        let data = &mut [0; Self::HEADER_SIZE];
        file.read_exact(data)?;
        let mut data = &data[..];

        convert_u32!(high_msg_num, data);
        let high_msg_num = basicreal_to_u32(high_msg_num);
        convert_u32!(low_msg_num, data);
        let low_msg_num = basicreal_to_u32(low_msg_num);
        convert_u32!(num_active_msgs, data);
        let num_active_msgs = basicreal_to_u32(num_active_msgs);
        convert_u32!(num_callers, data);
        let num_callers = basicreal_to_u32(num_callers);
        let lock_status = convert_str(data);

        Ok(Self {
            high_msg_num,
            low_msg_num,
            active_msgs: num_active_msgs,
            callers: num_callers,
            lock_status,
        })
    }
}
