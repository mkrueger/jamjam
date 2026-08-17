#![no_main]

use jamjam::pcboard::PCBoardMessage;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut reader = data;
    // A base file holds one message after the other, so keep reading.
    for _ in 0..64 {
        if PCBoardMessage::read(&mut reader).is_err() {
            break;
        }
    }
});
