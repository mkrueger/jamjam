//! Low-level access to physical JAM records.
//!
//! These functions expose storage details that ordinary callers should not
//! need. Physical headers include deleted and superseded records, while direct
//! mutation can change fields that higher-level operations normally maintain.

use super::{JamMessageBase, msg_header::JamMessageHeader};

/// Reads every physical header in JHR order.
///
/// Deleted and superseded records are included. Use
/// [`JamMessageBase::messages`] for the current live messages.
pub fn read_physical_headers(base: &JamMessageBase) -> crate::Result<Vec<JamMessageHeader>> {
    base.read_physical_headers()
}

/// Iterates every physical header in JHR order.
///
/// Deleted and superseded records are included. This API is primarily useful
/// for diagnostics and repair tools.
pub fn physical_headers(
    base: &JamMessageBase,
) -> crate::Result<impl Iterator<Item = crate::Result<JamMessageHeader>> + use<>> {
    base.physical_headers()
}

/// Changes raw attribute bits on an indexed message.
///
/// `ActiveMsgs` and `ModCounter` remain consistent when the deleted bit changes,
/// but callers are responsible for the semantic validity of all other bits.
pub fn set_attributes(
    base: &mut JamMessageBase,
    message_number: u32,
    set: u32,
    clear: u32,
) -> crate::Result<bool> {
    base.set_attributes(message_number, set, clear)
}

/// Replaces an indexed header while preserving its message number.
///
/// The replacement is appended physically and the old record is retired. The
/// caller is responsible for keeping reply links, text offsets, and subfields
/// semantically valid.
pub fn update_header(
    base: &mut JamMessageBase,
    message_number: u32,
    header: &JamMessageHeader,
) -> crate::Result<()> {
    base.update_header(message_number, header)
}
