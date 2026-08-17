use std::hint::black_box;
use std::path::Path;

use bstr::BString;
use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use jamjam::jam::msg_header::JamMessageHeader;
use jamjam::jam::pack::PackOptions;
use jamjam::jam::{JamMessage, JamMessageBase};
use jamjam::util::echomail::EchomailAddress;
use tempfile::TempDir;

const MESSAGES: u32 = 2_000;
const TEXT_LEN: usize = 200;

fn message(number: u32) -> JamMessage {
    JamMessage::new(&EchomailAddress::default())
        .with_from(BString::from("sysop"))
        .with_to(BString::from(if number.is_multiple_of(2) {
            "alice"
        } else {
            "bob"
        }))
        .with_subject(BString::from(format!("subject {number}")))
        .with_text(BString::from("x".repeat(TEXT_LEN)))
}

fn filled(path: &Path, count: u32) -> JamMessageBase {
    let mut base = JamMessageBase::create(path).unwrap();
    base.transaction(|base| {
        for number in 1..=count {
            base.write_message(&message(number))?;
        }
        Ok(())
    })
    .unwrap();
    base
}

/// Copies the four files a message base consists of, so a mutating benchmark
/// can start from the same state on every iteration.
fn copy_base(from: &Path, to: &Path) {
    for extension in ["jhr", "jdt", "jdx", "jlr"] {
        std::fs::copy(from.with_extension(extension), to.with_extension(extension)).unwrap();
    }
}

fn open(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("open");
    drop(filled(&path, MESSAGES));

    c.bench_function("open", |b| {
        b.iter(|| black_box(JamMessageBase::open(&path).unwrap()))
    });
}

fn read(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("read");
    let base = filled(&path, MESSAGES);

    let mut group = c.benchmark_group("read");
    group.bench_function("single_header", |b| {
        b.iter(|| black_box(base.read_header(MESSAGES / 2).unwrap()))
    });
    group.bench_function("single_message", |b| {
        b.iter(|| black_box(base.read_message(MESSAGES / 2).unwrap()))
    });

    group.throughput(Throughput::Elements(MESSAGES as u64));
    group.bench_function("all_headers", |b| {
        b.iter(|| {
            for header in base.messages() {
                black_box(header.unwrap());
            }
        })
    });
    group.bench_function("all_messages", |b| {
        b.iter(|| {
            for message in base.messages_full() {
                black_box(message.unwrap());
            }
        })
    });
    group.finish();
}

fn search(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("search");
    let base = filled(&path, MESSAGES);
    let msgid_crc = base.read_header(MESSAGES / 2).unwrap().msgid_crc;

    let mut group = c.benchmark_group("search");
    group.throughput(Throughput::Elements(MESSAGES as u64));
    group.bench_function("search_to", |b| {
        b.iter(|| black_box(base.search_to(&BString::from("alice")).unwrap()))
    });
    group.bench_function("find_by_msgid_crc", |b| {
        b.iter(|| black_box(base.find_by_msgid_crc(msgid_crc).unwrap()))
    });
    group.finish();
}

fn write(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("write");
    let mut base = filled(&path, 128);
    let mut number = 0;

    let mut group = c.benchmark_group("write");
    group.bench_function("append", |b| {
        b.iter(|| {
            number += 1;
            black_box(base.write_message(&message(number)).unwrap())
        })
    });
    group.bench_function("delete_and_restore", |b| {
        b.iter(|| {
            base.delete_message(1).unwrap();
            base.restore_message(1).unwrap();
        })
    });
    group.finish();
}

fn maintenance(c: &mut Criterion) {
    let template_dir = TempDir::new().unwrap();
    let template = template_dir.path().join("template");
    let mut base = filled(&template, MESSAGES);
    for number in (1..=MESSAGES).step_by(3) {
        base.delete_message(number).unwrap();
    }
    drop(base);

    let mut group = c.benchmark_group("maintenance");
    group.throughput(Throughput::Elements(MESSAGES as u64));

    let prepare = || {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("work");
        copy_base(&template, &path);
        let base = JamMessageBase::open(&path).unwrap();
        (dir, base)
    };

    group.bench_function("pack", |b| {
        b.iter_batched(
            prepare,
            |(dir, mut base)| {
                black_box(base.pack(&PackOptions::default()).unwrap());
                dir
            },
            BatchSize::SmallInput,
        )
    });
    group.bench_function("reindex", |b| {
        b.iter_batched(
            prepare,
            |(dir, mut base)| {
                black_box(base.reindex().unwrap());
                dir
            },
            BatchSize::SmallInput,
        )
    });

    let (_dir, mut base) = prepare();
    group.bench_function("verify", |b| b.iter(|| black_box(base.verify().unwrap())));
    group.finish();
}

fn header_codec(c: &mut Criterion) {
    let header = message(1).header().clone();
    let mut encoded = Vec::new();
    header.write(&mut encoded).unwrap();

    let mut group = c.benchmark_group("header");
    group.throughput(Throughput::Bytes(encoded.len() as u64));
    group.bench_function("write", |b| {
        b.iter(|| {
            let mut buffer = Vec::with_capacity(encoded.len());
            header.write(&mut buffer).unwrap();
            black_box(buffer)
        })
    });
    group.bench_function("read", |b| {
        b.iter(|| black_box(JamMessageHeader::read(&mut encoded.as_slice()).unwrap()))
    });
    group.finish();
}

criterion_group!(
    benches,
    open,
    read,
    search,
    write,
    maintenance,
    header_codec
);
criterion_main!(benches);
