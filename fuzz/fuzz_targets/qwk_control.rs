#![no_main]

use bstr::ByteSlice;
use jamjam::qwk::control::ControlDat;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(control) = ControlDat::read(data) else {
        return;
    };

    let again =
        ControlDat::read(&control.to_vec()).expect("a generated CONTROL.DAT must parse again");
    // Lines are written with a plain newline, so a value ending in CR reads
    // back as if it had been the CR of a CRLF pair.
    assert_eq!(again.bbs_name.trim_end(), control.bbs_name.trim_end());
    assert_eq!(again.bbs_id.trim_end(), control.bbs_id.trim_end());
    assert_eq!(again.serial_number, control.serial_number);
    assert_eq!(again.message_count, control.message_count);
    assert_eq!(again.conferences.len(), control.conferences.len());
    assert_eq!(again.extra_lines.len(), control.extra_lines.len());
});
