//! Reading, writing and converting BBS message bases.
//!
//! The crate covers the [`jam`] message base, the [`pcboard`] message base it
//! grew out of and [`qwk`] mail packets. Every fallible call returns [`Result`],
//! which separates I/O failures from malformed data so that a corrupt base is
//! reported instead of looking like a short file.

#[macro_use]
pub(crate) mod macros;

pub mod conversion;
mod error;
pub mod jam;
pub mod pcboard;
pub mod qwk;
pub mod util;

pub use error::{Error, Format, Result};
