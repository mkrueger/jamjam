#![no_main]

use jamjam::qwk::QwkMessageBase;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(records) = QwkMessageBase::convert_qwk_index(data) {
        assert_eq!(records.len(), data.len() / 5);
    }
});
