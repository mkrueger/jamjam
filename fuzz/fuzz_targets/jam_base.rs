#![no_main]

use arbitrary::Arbitrary;
use bstr::BString;
use jamjam::jam::JamMessageBase;
use libfuzzer_sys::fuzz_target;
use tempfile::TempDir;

/// The four files of a message base, generated independently of each other.
#[derive(Arbitrary, Debug)]
struct BaseFiles {
    jhr: Vec<u8>,
    jdt: Vec<u8>,
    jdx: Vec<u8>,
    jlr: Vec<u8>,
    repair: bool,
}

fuzz_target!(|files: BaseFiles| {
    let Ok(dir) = TempDir::new() else {
        return;
    };
    let path = dir.path().join("fuzz");
    for (extension, bytes) in [
        ("jhr", &files.jhr),
        ("jdt", &files.jdt),
        ("jdx", &files.jdx),
        ("jlr", &files.jlr),
    ] {
        if std::fs::write(path.with_extension(extension), bytes).is_err() {
            return;
        }
    }

    let Ok(mut base) = JamMessageBase::open(&path) else {
        return;
    };

    let _ = base.verify();
    let low = base.lowest_message_number();
    let high = base.highest_message_number();
    for header in base.messages().flatten() {
        assert!((low..=high).contains(&header.message_number));
    }
    for message in base.messages_full() {
        let _ = message;
    }
    let _ = base.read_last_read_file();
    let _ = base.search_to(&BString::from("all"));

    // A base that repair calls clean has to read back end to end.
    if files.repair
        && let Ok(report) = base.repair()
        && report.after.is_ok()
    {
        for message in base.messages_full() {
            message.expect("a clean base must not hold unreadable messages");
        }
    }
});
