use bstr::BString;
use jamjam::jam::pack::PackOptions;
use jamjam::jam::{JamMessage, JamMessageBase};
use jamjam::util::echomail::EchomailAddress;
use pretty_assertions::assert_eq;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn message(text: &str) -> JamMessage {
    JamMessage::new(&EchomailAddress::default())
        .with_to(BString::from("all"))
        .with_subject(BString::from(text))
        .with_text(BString::from(text))
}

fn filled(path: &Path) -> JamMessageBase {
    let mut base = JamMessageBase::create(path).unwrap();
    for text in ["one", "two", "three"] {
        base.write_message(&message(text)).unwrap();
    }
    base
}

fn files(path: &Path) -> [PathBuf; 3] {
    [
        path.with_extension("jhr"),
        path.with_extension("jdt"),
        path.with_extension("jdx"),
    ]
}

fn snapshot(path: &Path) -> [Vec<u8>; 3] {
    files(path).map(|file| std::fs::read(file).unwrap())
}

fn assert_unchanged(path: &Path, before: &[Vec<u8>; 3]) {
    for (file, expected) in files(path).iter().zip(before) {
        assert_eq!(
            &std::fs::read(file).unwrap(),
            expected,
            "{} changed",
            file.display()
        );
    }
}

fn write_at(path: &Path, offset: u64, data: &[u8]) {
    let mut file = OpenOptions::new().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(data).unwrap();
}

#[test]
fn test_read_headers_rejects_a_truncated_physical_record() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("truncated");
    let base = filled(&path);
    let header_path = path.with_extension("jhr");
    let len = std::fs::metadata(&header_path).unwrap().len();
    OpenOptions::new()
        .write(true)
        .open(header_path)
        .unwrap()
        .set_len(len - 1)
        .unwrap();

    assert!(jamjam::jam::raw::read_physical_headers(&base).is_err());
}

#[test]
fn test_read_headers_rejects_corruption_in_the_middle() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("middle");
    let base = filled(&path);
    let second = base.read_header(2).unwrap();
    let first = base.read_header(1).unwrap();
    let second_offset = 1024
        + 76
        + first
            .sub_fields
            .iter()
            .map(|field| 8 + field.content().len())
            .sum::<usize>();
    assert_eq!(second.message_number, 2);
    write_at(&path.with_extension("jhr"), second_offset as u64, b"NOPE");

    assert!(jamjam::jam::raw::read_physical_headers(&base).is_err());
}

#[test]
fn test_pack_rejects_a_corrupt_indexed_header_without_rewriting_files() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("header");
    let mut base = filled(&path);
    write_at(&path.with_extension("jhr"), 1024, b"NOPE");
    let before = snapshot(&path);

    assert!(base.pack(&PackOptions::default()).is_err());
    assert_unchanged(&path, &before);
}

#[test]
fn test_pack_rejects_text_outside_jdt_without_rewriting_files() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("text");
    let mut base = filled(&path);
    let text_path = path.with_extension("jdt");
    let len = std::fs::metadata(&text_path).unwrap().len();
    OpenOptions::new()
        .write(true)
        .open(&text_path)
        .unwrap()
        .set_len(len - 1)
        .unwrap();
    let before = snapshot(&path);

    assert!(base.pack(&PackOptions::default()).is_err());
    assert_unchanged(&path, &before);
}

#[test]
fn test_pack_rejects_an_index_offset_outside_jhr() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("offset");
    let mut base = filled(&path);
    write_at(&path.with_extension("jdx"), 4, &u32::MAX.to_le_bytes());
    let before = snapshot(&path);

    assert!(base.pack(&PackOptions::default()).is_err());
    assert_unchanged(&path, &before);
}

#[test]
fn test_pack_rejects_a_half_empty_index_record() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("index");
    let mut base = filled(&path);
    write_at(&path.with_extension("jdx"), 0, &0u32.to_le_bytes());
    write_at(&path.with_extension("jdx"), 4, &u32::MAX.to_le_bytes());
    let before = snapshot(&path);

    assert!(base.pack(&PackOptions::default()).is_err());
    assert_unchanged(&path, &before);
}

#[test]
fn test_renumber_rejects_truncated_lastread_before_commit() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("lastread");
    let mut base = filled(&path);
    std::fs::write(path.with_extension("jlr"), [0u8; 20]).unwrap();
    let messages_before = snapshot(&path);
    let lastread_before = std::fs::read(path.with_extension("jlr")).unwrap();

    let result = base.pack(&PackOptions {
        renumber_from: Some(500),
        ..Default::default()
    });

    assert!(result.is_err());
    assert_unchanged(&path, &messages_before);
    assert_eq!(
        std::fs::read(path.with_extension("jlr")).unwrap(),
        lastread_before
    );
    assert_eq!(base.lowest_message_number(), 1);
    assert_eq!(base.highest_message_number(), 3);
    assert!(!path.with_extension("jampack").exists());
}

#[test]
fn test_reindex_rejects_a_truncated_header_without_rewriting_index() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("reindex");
    let mut base = filled(&path);
    let original_index = std::fs::read(path.with_extension("jdx")).unwrap();
    let header_path = path.with_extension("jhr");
    let len = std::fs::metadata(&header_path).unwrap().len();
    OpenOptions::new()
        .write(true)
        .open(header_path)
        .unwrap()
        .set_len(len - 1)
        .unwrap();

    assert!(base.reindex().is_err());
    assert_eq!(
        std::fs::read(path.with_extension("jdx")).unwrap(),
        original_index
    );
}

#[test]
fn test_messages_rejects_index_header_number_mismatch() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("number-mismatch");
    let base = filled(&path);
    // MessageNumber is the tenth ulong after the eight-byte signature/revision.
    write_at(&path.with_extension("jhr"), 1024 + 48, &99u32.to_le_bytes());

    let first = base.messages().next().unwrap();
    assert!(first.is_err());
}
