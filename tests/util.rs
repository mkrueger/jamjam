use jamjam::util::basic_real::BasicReal;
use jamjam::util::cp437::to_utf8;
use jamjam::util::echomail::EchomailAddress;
use pretty_assertions::assert_eq;

#[test]
fn test_basicreal_conversion() {
    for i in -100..100 {
        assert_eq!(i, BasicReal::from(i).into());
    }
}

#[test]
fn test_ascii_is_passed_through() {
    assert_eq!(to_utf8(b"Hello"), "Hello");
}

#[test]
fn test_high_bytes_fall_back_to_cp437() {
    // A continuation byte cannot start a UTF-8 sequence, so this is CP437.
    assert_eq!(to_utf8(&[0xB1, 0xDB]), "\u{2592}\u{2588}");
}

#[test]
fn test_utf8_is_recognized() {
    assert_eq!(to_utf8("Grüße".as_bytes()), "Grüße");
}

#[test]
fn test_bom_is_stripped() {
    assert_eq!(to_utf8(&[0xEF, 0xBB, 0xBF, b'h', b'i']), "hi");
}

#[test]
fn test_echomail_address_display() {
    let addr = EchomailAddress::new(1, 2, 3, 4);
    assert_eq!(addr.to_string(), "1:2/3.4");

    let addr = EchomailAddress::new(1, 2, 3, 0);
    assert_eq!(addr.to_string(), "1:2/3");
}

#[test]
fn test_echomail_address_parse() {
    let addr = EchomailAddress::parse("1:2/3.4").unwrap();
    assert_eq!(addr.zone, 1);
    assert_eq!(addr.net, 2);
    assert_eq!(addr.node, 3);
    assert_eq!(addr.point, 4);

    let addr = EchomailAddress::parse("1:2/3").unwrap();
    assert_eq!(addr.zone, 1);
    assert_eq!(addr.net, 2);
    assert_eq!(addr.node, 3);
    assert_eq!(addr.point, 0);

    assert!(EchomailAddress::parse("1:2/3.4.5").is_none());
    assert!(EchomailAddress::parse("1:2").is_none());
    assert!(EchomailAddress::parse("1:2/3.").is_none());
}
