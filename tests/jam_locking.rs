use bstr::BString;
use jamjam::jam::last_read_storage::JamLastReadStorage;
use jamjam::jam::pack::PackOptions;
use jamjam::jam::{JamMessage, JamMessageBase};
use jamjam::util::echomail::EchomailAddress;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::time::Duration;
use tempfile::TempDir;

fn message(text: &str) -> JamMessage {
    JamMessage::new(&EchomailAddress::default())
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

#[test]
fn test_shared_locks_allow_several_readers() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("shared");
    let mut first = filled(&path);
    let mut second = JamMessageBase::open(&path).unwrap();
    let mut writer = JamMessageBase::open(&path).unwrap();

    first.lock_shared().unwrap();
    assert!(second.try_lock_shared().unwrap());
    assert!(!writer.try_lock().unwrap(), "a writer slipped past readers");
    assert_eq!(first.active_messages(), 3);
    assert_eq!(second.active_messages(), 3);

    first.unlock();
    second.unlock();
    assert!(writer.try_lock().unwrap());
    writer.unlock();
}

#[test]
fn test_exclusive_lock_blocks_shared_readers() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("exclusive");
    let mut writer = filled(&path);
    let mut reader = JamMessageBase::open(&path).unwrap();

    writer.lock().unwrap();
    assert!(!reader.try_lock_shared().unwrap());
    writer.unlock();
    assert!(reader.try_lock_shared().unwrap());
    reader.unlock();
}

#[test]
fn test_shared_lock_cannot_be_upgraded() {
    let dir = TempDir::new().unwrap();
    let mut base = filled(&dir.path().join("upgrade"));

    base.lock_shared().unwrap();
    let err = base.write_message(&message("four")).unwrap_err();
    match err {
        jamjam::Error::Jam(jamjam::jam::JamError::LockUpgrade) => {}
        other => panic!("expected LockUpgrade, got {other}"),
    }
    base.unlock();
    assert_eq!(base.write_message(&message("four")).unwrap(), 4);
}

#[test]
fn test_read_transaction_refreshes_and_releases() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("snapshot");
    let mut writer = filled(&path);
    let mut reader = JamMessageBase::open(&path).unwrap();

    writer.write_message(&message("four")).unwrap();
    let count = reader
        .read_transaction(|base| Ok(base.active_messages()))
        .unwrap();
    assert_eq!(count, 4);
    assert!(writer.try_lock().unwrap());
    writer.unlock();
}

#[test]
fn test_stable_lock_survives_header_replacement() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("stable");
    let mut base = filled(&path);

    base.lock().unwrap();
    base.pack(&PackOptions::default()).unwrap();

    let (sender, receiver) = mpsc::channel();
    let open_path = path.clone();
    let opener = std::thread::spawn(move || {
        sender.send(JamMessageBase::open(open_path)).unwrap();
    });

    assert!(receiver.recv_timeout(Duration::from_millis(100)).is_err());
    base.unlock();
    assert!(
        receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .is_ok()
    );
    opener.join().unwrap();
}

#[test]
fn test_alternating_handles_refresh_counters_and_numbers() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("alternating");
    let mut first = filled(&path);
    let mut second = JamMessageBase::open(&path).unwrap();

    assert_eq!(first.write_message(&message("four")).unwrap(), 4);
    assert_eq!(second.write_message(&message("five")).unwrap(), 5);
    first.delete_message(1).unwrap();

    let reopened = JamMessageBase::open(&path).unwrap();
    assert_eq!(reopened.active_messages(), 4);
    assert_eq!(reopened.highest_message_number(), 5);
    assert_eq!(texts(&reopened), vec!["two", "three", "four", "five"]);
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

#[test]
fn test_transaction_releases_the_lock_on_panic() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("panic");
    let mut base = filled(&path);
    let mut other = JamMessageBase::open(&path).unwrap();

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = base.transaction::<()>(|_| panic!("injected transaction panic"));
    }));
    assert!(panic.is_err());
    assert!(other.try_lock().unwrap());
    other.unlock();
}

#[test]
fn test_failed_append_rolls_back_text_header_and_cached_state() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("rollback");
    let mut base = filled(&path);
    let header_path = path.with_extension("jhr");
    let text_path = path.with_extension("jdt");
    let index_path = path.with_extension("jdx");
    let backup_index = path.with_extension("jdx.backup");
    let header_len = std::fs::metadata(&header_path).unwrap().len();
    let text_len = std::fs::metadata(&text_path).unwrap().len();
    let mod_counter = base.mod_counter();

    let result = base.transaction(|base| {
        std::fs::rename(&index_path, &backup_index)?;
        std::fs::create_dir(&index_path)?;
        let result = base.write_message(&message("must roll back"));
        std::fs::remove_dir(&index_path)?;
        std::fs::rename(&backup_index, &index_path)?;
        result.map(|_| ())
    });

    assert!(result.is_err());
    assert_eq!(std::fs::metadata(header_path).unwrap().len(), header_len);
    assert_eq!(std::fs::metadata(text_path).unwrap().len(), text_len);
    assert_eq!(base.active_messages(), 3);
    assert_eq!(base.highest_message_number(), 3);
    assert_eq!(base.mod_counter(), mod_counter);
    assert_eq!(texts(&base), vec!["one", "two", "three"]);
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
fn test_interrupted_renumber_finishes_lastread_on_open() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("lastread-crash");
    let mut base = filled(&path);
    base.write_last_read(&JamLastReadStorage {
        user_crc: 1,
        user_id: 2,
        last_read_msg: 3,
        high_read_msg: 3,
    })
    .unwrap();
    let old_lastread = std::fs::read(path.with_extension("jlr")).unwrap();

    base.pack(&PackOptions::default().with_renumber_from(500))
        .unwrap();
    let remapped_lastread = std::fs::read(path.with_extension("jlr")).unwrap();
    drop(base);

    // Recreate a crash after JDT/JHR/JDX were moved but before JLR was moved.
    std::fs::write(path.with_extension("jlr"), old_lastread).unwrap();
    std::fs::write(path.with_extension("jlr.packing"), remapped_lastread).unwrap();
    std::fs::write(path.with_extension("jampack"), b"jamjam pack in progress\n").unwrap();

    let base = JamMessageBase::open(&path).unwrap();
    let lastread = base.find_last_read(1, 2).unwrap().unwrap();

    assert_eq!(base.lowest_message_number(), 500);
    assert_eq!(base.highest_message_number(), 502);
    assert_eq!(lastread.last_read_msg, 502);
    assert_eq!(lastread.high_read_msg, 502);
    assert!(!path.with_extension("jampack").exists());
    assert!(!path.with_extension("jlr.packing").exists());
}

#[test]
fn test_pack_leaves_no_marker_behind() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("clean");
    let mut base = filled(&path);
    base.pack(&PackOptions::default()).unwrap();

    assert!(!path.with_extension("jampack").exists());
    assert!(!path.with_extension("jdt.packing").exists());
    assert!(!path.with_extension("jlr.packing").exists());
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

#[test]
fn test_create_refuses_to_overwrite_an_existing_base() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("existing");
    let base = filled(&path);
    let before = texts(&base);
    drop(base);

    assert!(JamMessageBase::create(&path).is_err());

    let reopened = JamMessageBase::open(path).unwrap();
    assert_eq!(texts(&reopened), before);
    assert_eq!(reopened.active_messages(), 3);
}

#[test]
fn test_concurrent_create_has_one_winner() {
    let dir = TempDir::new().unwrap();
    let path = Arc::new(dir.path().join("concurrent-create"));
    let barrier = Arc::new(Barrier::new(3));
    let mut threads = Vec::new();

    for _ in 0..2 {
        let path = Arc::clone(&path);
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            JamMessageBase::create(path.as_path()).is_ok()
        }));
    }
    barrier.wait();

    let winners = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .filter(|won| *won)
        .count();
    assert_eq!(winners, 1);
    assert!(JamMessageBase::open(path.as_path()).is_ok());
}
