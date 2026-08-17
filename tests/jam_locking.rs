use bstr::BString;
use jamjam::jam::pack::PackOptions;
use jamjam::jam::{JamMessage, JamMessageBase};
use jamjam::util::echomail::EchomailAddress;
use pretty_assertions::assert_eq;
use std::path::Path;
use tempfile::TempDir;

fn message(text: &str) -> JamMessage {
    JamMessage::new(0, &EchomailAddress::default())
        .with_to(BString::from("all"))
        .with_text(BString::from(text))
}

fn filled(path: &Path) -> JamMessageBase {
    let mut base = JamMessageBase::create(path).unwrap();
    for text in ["one", "two", "three"] {
        base.write_message(&message(text)).unwrap();
    }
    base
}

fn texts(base: &JamMessageBase) -> Vec<String> {
    (base.lowest_message_number()..=base.highest_message_number())
        .filter_map(|no| base.read_header(no).ok())
        .map(|header| base.read_message_text(&header).unwrap().to_string())
        .collect()
}

/// The lock is advisory and shared between programs, so a second handle on the
/// same base must not be able to take it while the first one holds it.
#[test]
fn test_lock_excludes_another_handle() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("locked");
    let mut base = filled(&path);
    let mut other = JamMessageBase::open(&path).unwrap();

    base.lock().unwrap();
    assert!(!other.try_lock().unwrap(), "the second handle got the lock");
    // Reading does not take the lock, so it still works.
    assert_eq!(other.active_messages(), 3);

    base.unlock();
    assert!(other.try_lock().unwrap(), "the lock was not released");
    other.unlock();
}

/// A mutating call has to hold the lock for its whole duration.
#[test]
fn test_writes_hold_the_lock() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("held");
    let mut base = filled(&path);
    let mut other = JamMessageBase::open(&path).unwrap();

    base.transaction(|base| {
        base.write_message(&message("four"))?;
        assert!(!other.try_lock().unwrap(), "a write left the base unlocked");
        Ok(())
    })
    .unwrap();

    assert!(other.try_lock().unwrap());
    other.unlock();
}

#[test]
fn test_lock_nests() {
    let dir = TempDir::new().unwrap();
    let mut base = filled(&dir.path().join("nested"));

    base.lock().unwrap();
    base.lock().unwrap();
    // A mutating call locks on its own, which must not release the outer lock.
    base.write_message(&message("four")).unwrap();
    base.unlock();
    base.unlock();

    // Unbalanced unlocking must not underflow.
    base.unlock();
    assert_eq!(base.active_messages(), 4);
}

#[test]
fn test_transaction_groups_writes() {
    let dir = TempDir::new().unwrap();
    let mut base = filled(&dir.path().join("txn"));

    base.transaction(|base| {
        base.write_message(&message("four"))?;
        base.delete_message(1)?;
        Ok(())
    })
    .unwrap();

    assert_eq!(base.active_messages(), 3);
    assert_eq!(base.highest_message_number(), 4);
}

#[test]
fn test_transaction_releases_the_lock_on_error() {
    let dir = TempDir::new().unwrap();
    let mut base = filled(&dir.path().join("txn"));

    let failed = base.transaction(|base| {
        base.write_message(&message("four"))?;
        base.read_header(999)?;
        Ok(())
    });
    assert!(failed.is_err());

    // Still usable, so the lock came back.
    base.write_message(&message("five")).unwrap();
    assert_eq!(base.highest_message_number(), 5);
}

/// A pack that dies between the renames used to leave the new text file next to
/// the old headers, which points every message at the wrong bytes.
#[test]
fn test_interrupted_pack_is_completed_on_open() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("crash");
    let mut base = filled(&path);
    base.delete_message(2).unwrap();
    base.pack(&PackOptions::default()).unwrap();
    let packed = texts(&base);
    drop(base);

    // Rewind to the state a crash between the first and second rename leaves:
    // the marker is there and some temporaries are still waiting.
    let text = path.with_extension("jdt");
    let header = path.with_extension("jhr");
    let index = path.with_extension("jdx");
    for (target, tmp) in [&text, &header, &index].iter().map(|p| {
        let mut tmp = p.as_os_str().to_os_string();
        tmp.push(".packing");
        (*p, std::path::PathBuf::from(tmp))
    }) {
        std::fs::rename(target, &tmp).unwrap();
    }
    // The text file made it across before the crash.
    let mut text_tmp = text.as_os_str().to_os_string();
    text_tmp.push(".packing");
    std::fs::rename(std::path::PathBuf::from(text_tmp), &text).unwrap();
    std::fs::write(path.with_extension("jampack"), b"jamjam pack in progress\n").unwrap();

    let base = JamMessageBase::open(&path).unwrap();

    assert!(!path.with_extension("jampack").exists());
    assert_eq!(texts(&base), packed);
    assert_eq!(base.active_messages(), 2);
}

#[test]
fn test_pack_leaves_no_marker_behind() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("clean");
    let mut base = filled(&path);
    base.pack(&PackOptions::default()).unwrap();

    assert!(!path.with_extension("jampack").exists());
    assert!(!path.with_extension("jdt.packing").exists());
}

#[test]
fn test_sync_succeeds_on_a_fresh_base() {
    let dir = TempDir::new().unwrap();
    let mut base = filled(&dir.path().join("sync"));
    base.write_message(&message("four")).unwrap();
    base.sync().unwrap();

    let reopened = JamMessageBase::open(dir.path().join("sync")).unwrap();
    assert_eq!(reopened.active_messages(), 4);
}

#[test]
fn test_delete_message_base_removes_every_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("gone");
    let base = filled(&path);
    base.delete_message_base().unwrap();

    for extension in ["jhr", "jdt", "jdx", "jlr"] {
        assert!(!path.with_extension(extension).exists());
    }
}
