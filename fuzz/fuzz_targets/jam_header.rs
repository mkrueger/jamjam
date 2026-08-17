#![no_main]

use jamjam::jam::msg_header::JamMessageHeader;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(header) = JamMessageHeader::read(&mut &data[..]) else {
        return;
    };

    let mut encoded = Vec::new();
    header
        .write(&mut encoded)
        .expect("an accepted header must encode again");
    let again = JamMessageHeader::read(&mut encoded.as_slice())
        .expect("an encoded header must parse again");

    assert_eq!(again.message_number, header.message_number);
    assert_eq!(again.offset, header.offset);
    assert_eq!(again.txt_len, header.txt_len);
    assert_eq!(again.attributes, header.attributes);
    assert_eq!(again.sub_fields.len(), header.sub_fields.len());
    for (left, right) in again.sub_fields.iter().zip(&header.sub_fields) {
        assert_eq!(left.field_type(), right.field_type());
        assert_eq!(left.content(), right.content());
    }
});
