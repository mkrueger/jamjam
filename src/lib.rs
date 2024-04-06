#![feature(buf_read_has_data_left)]

pub mod conversion;
pub mod jam;
mod macros;
pub mod pcboard;
pub mod util;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
