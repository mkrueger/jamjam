//! The message base lock is an advisory file lock, so it only means something
//! between processes. These tests spawn the test binary again to check that a
//! second process is really kept out, and that concurrent writers do not lose
//! messages.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use bstr::BString;
use jamjam::jam::{JamMessage, JamMessageBase};
use jamjam::util::echomail::EchomailAddress;
use tempfile::TempDir;

const MODE: &str = "JAMJAM_CROSS_PROCESS_MODE";
const BASE: &str = "JAMJAM_CROSS_PROCESS_BASE";
const ID: &str = "JAMJAM_CROSS_PROCESS_ID";

const MESSAGES_PER_WRITER: u32 = 15;
const WRITERS: u32 = 3;

fn message(text: &str) -> JamMessage {
    JamMessage::new(&EchomailAddress::default())
        .with_to(BString::from("all"))
        .with_subject(BString::from(text))
        .with_text(BString::from(text))
}

fn held(path: &Path) -> PathBuf {
    path.with_extension("held")
}

fn release(path: &Path) -> PathBuf {
    path.with_extension("release")
}

/// The other half of these tests, running in the spawned process.
#[test]
#[ignore = "spawned by the cross process tests"]
fn lock_helper() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    let path = PathBuf::from(std::env::var(BASE).unwrap());
    let mut base = JamMessageBase::open(&path).unwrap();

    if mode == "write" {
        let id = std::env::var(ID).unwrap();
        for index in 0..MESSAGES_PER_WRITER {
            base.write_message(&message(&format!("{id}-{index}")))
                .unwrap();
        }
        return;
    }

    match mode.as_str() {
        "exclusive" => base.lock().unwrap(),
        "shared" => base.lock_shared().unwrap(),
        other => panic!("unknown mode {other}"),
    }
    std::fs::write(held(&path), []).unwrap();
    wait_for(&release(&path));
    base.unlock();
}

fn spawn(mode: &str, path: &Path) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lock_helper", "--ignored"])
        .env(MODE, mode)
        .env(BASE, path)
        // Keep the harness chatter of the child out, its panics still show.
        .stdout(Stdio::null())
        .spawn()
        .unwrap()
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn finish(mut child: Child, path: &Path) {
    std::fs::write(release(path), []).unwrap();
    assert!(child.wait().unwrap().success(), "the helper process failed");
}

fn filled(path: &Path) {
    let mut base = JamMessageBase::create(path).unwrap();
    base.write_message(&message("one")).unwrap();
}

#[test]
fn test_an_exclusive_lock_in_another_process_keeps_everyone_out() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("exclusive");
    filled(&path);
    let mut base = JamMessageBase::open(&path).unwrap();

    let child = spawn("exclusive", &path);
    wait_for(&held(&path));

    assert!(!base.try_lock().unwrap(), "a second writer got the lock");
    assert!(
        !base.try_lock_shared().unwrap(),
        "a reader got past a writer"
    );

    finish(child, &path);

    assert!(base.try_lock().unwrap(), "the lock outlived the process");
    base.unlock();
}

#[test]
fn test_a_shared_lock_in_another_process_lets_readers_in() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("shared");
    filled(&path);
    let mut base = JamMessageBase::open(&path).unwrap();

    let child = spawn("shared", &path);
    wait_for(&held(&path));

    assert!(
        base.try_lock_shared().unwrap(),
        "two readers cannot read at the same time"
    );
    base.unlock();
    assert!(!base.try_lock().unwrap(), "a writer got past a reader");

    finish(child, &path);

    assert!(base.try_lock().unwrap());
    base.unlock();
}

/// Opening runs the crash recovery, so it has to wait for a process that is
/// packing rather than look at half moved files.
#[test]
fn test_opening_waits_for_a_writer_in_another_process() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("waiting");
    filled(&path);

    let child = spawn("exclusive", &path);
    wait_for(&held(&path));

    let opening = thread::spawn({
        let path = path.clone();
        move || JamMessageBase::open(&path).map(|_| ())
    });
    thread::sleep(Duration::from_millis(200));
    assert!(
        !opening.is_finished(),
        "opening did not wait for the lock holder"
    );

    finish(child, &path);
    opening.join().unwrap().unwrap();
}

/// Every writer takes the lock for itself, so the numbers they hand out have to
/// stay unique even though none of them knows about the others.
#[test]
fn test_writers_in_other_processes_do_not_lose_messages() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("writers");
    drop(JamMessageBase::create(&path).unwrap());

    let children: Vec<Child> = (0..WRITERS)
        .map(|id| {
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "lock_helper", "--ignored"])
                .env(MODE, "write")
                .env(BASE, &path)
                .env(ID, id.to_string())
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success(), "a writer process failed");
    }

    let mut base = JamMessageBase::open(&path).unwrap();
    let expected = WRITERS * MESSAGES_PER_WRITER;
    assert_eq!(base.active_messages(), expected);

    let report = base.verify().unwrap();
    assert!(report.is_ok(), "{report}");
    assert_eq!(report.live_messages, expected);

    let numbers: BTreeSet<u32> = base
        .messages()
        .map(|header| header.unwrap().message_number)
        .collect();
    assert_eq!(numbers.len(), expected as usize);

    let texts: BTreeSet<String> = base
        .messages_full()
        .map(|message| message.unwrap().text().to_string())
        .collect();
    let all: BTreeSet<String> = (0..WRITERS)
        .flat_map(|id| (0..MESSAGES_PER_WRITER).map(move |index| format!("{id}-{index}")))
        .collect();
    assert_eq!(texts, all);
}
