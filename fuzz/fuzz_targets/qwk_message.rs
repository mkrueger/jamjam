#![no_main]

use bstr::ByteSlice;
use jamjam::qwk::qwk_message::QwkMessage;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    for extended in [false, true] {
        let Ok(message) = QwkMessage::read(data, extended) else {
            continue;
        };

        let mut encoded = Vec::new();
        // Fields wider than their fixed slot are rejected on write.
        if message.write(&mut encoded, extended).is_err() {
            continue;
        }
        let again = QwkMessage::read(encoded.as_slice(), extended)
            .expect("an encoded message must parse again");

        // Text that starts like a kludge line is read back as one, which the
        // block padding can also turn a bare "Subject:" into.
        if extended && looks_like_kludge(&message.text) {
            continue;
        }
        // The fixed slots are padded with spaces, so trailing ones do not survive.
        assert_eq!(again.to.trim_end(), message.to.trim_end());
        assert_eq!(again.from.trim_end(), message.from.trim_end());
        assert_eq!(again.subj.trim_end(), message.subj.trim_end());
        assert_eq!(again.msg_number, message.msg_number);
        assert_eq!(again.conference_number, message.conference_number);
    }
});

fn looks_like_kludge(text: &[u8]) -> bool {
    text.starts_with(b"To:") || text.starts_with(b"From:") || text.starts_with(b"Subject:")
}
