# jamjam

[![CI](https://github.com/mkrueger/jamjam/actions/workflows/ci.yml/badge.svg)](https://github.com/mkrueger/jamjam/actions/workflows/ci.yml)

A library to read, write and convert BBS message bases. It started as a library for the JAM message base format, then grew PCBoard and QWK support because the same project needed them.

## Formats

| Format | Read | Write | Notes |
| ------ | ---- | ----- | ----- |
| JAM | yes | yes | Including packing, renumbering and index rebuilding |
| PCBoard | yes | no | Reading and conversion only, the format is documented in `doc/` if you want to add writing |
| QWK / QWKE | yes | yes | Messages and `CONTROL.DAT` |

## JAM

From Wikipedia:

> The JAM Message Base Format was one of the most popular file formats of message bases on DOS-based BBSes in the 1990s. JAM stands for "Joaquim-Andrew-Mats" after the original authors of the API, Joaquim Homrighausen, Andrew Milner, Mats Birch, and Mats Wallin.[1] Joaquim was the author of FrontDoor, a DOS-based FidoNet-compatible mailer. Andrew was the author of RemoteAccess, a popular DOS-based Bulletin Board System. JAM was originally released in 1993 in C, however the most popular implementation was Mark May's "MK Source for Msg Access" written in Pascal which also saw its initial release in 1993.

### Message numbering

The format keeps two separate values and mixing them up is an easy mistake:

- `active_messages()` is the number of messages that are not deleted.
- `lowest_message_number()` / `highest_message_number()` bracket the numbers the index can address.

A packed base can start at message 500 and a base with deleted messages has gaps, so message numbers are derived from the index rather than from the active count.

```rust
use jamjam::jam::{JamMessage, JamMessageBase};
use jamjam::util::echomail::EchomailAddress;

let mut base = JamMessageBase::create("mybase")?;
let number = base.write_message(
    &JamMessage::new(&EchomailAddress::default())
        .with_from("sysop".into())
        .with_to("all".into())
        .with_subject("hello".into())
        .with_text("world".into()),
)?;

let header = base.read_header(number)?;
println!("{}", base.read_message_text(&header)?);
# Ok::<(), jamjam::Error>(())
```

Use `base.messages()` to walk the current live messages in message-number
order, or `base.read_message(n)` / `base.messages_full()` when the text is
needed as well. Readers that need a consistent snapshot should take
`lock_shared()` (or `read_transaction`) so writers are kept out; writers
still take the exclusive lock. `verify()` reports consistency problems
without writing; `repair()` rebuilds the index and clamps last-read
pointers when asked. `search_to()` and `find_by_msgid()` look messages up
by recipient or MSGID.

Low-level operations that expose retired physical JHR records or mutate
raw header data live under `jamjam::jam::raw`; they are intended for diagnostics,
conversion and repair tools rather than normal message-base access.

### JAM copyright

jamjam is licensed under MIT-X11 or Apache 2.0 (your choice). JAM itself is from:

JAM(mbp) - Copyright 1993 Joaquim Homrighausen, Andrew Milner,
                               Mats Birch, Mats Wallin.
                               ALL RIGHTS RESERVED.

Note that jamjam doesn't contain any 3rd party source code but took some source code comments out of the official JAM (JAM.txt) document.

## PCBoard

This message format is only used inside PCBoard - an old bulletin board system. This format is the base of the more well known QWK standard. The QWK format inherits some funny things from that - for example the 'password' field which is only used by PCBoard AFAIK.

The goal is to read and understand the PCBoard format in order to convert it to something else. Writing is not implemented, but the format is documented and contributions are welcome.

## QWK

Reading and writing QWK bases works, including the QWKE extensions that carry over-long `To`, `From` and `Subject` values in kludge lines.

The `*.ndx` files are close to useless in practice: there is an off-by-one error in current QWK implementations. jamjam does the correct thing, because the program that defines QWK is PCBoard, and PCBoard doesn't count the first record in `messages.dat`, so 0 means the second record in the file. Set `index_offset_bug` on the message base to read packets written by implementations that got this wrong.

## Errors

Every fallible call returns `jamjam::Result<T>`, which carries a `jamjam::Error`. It distinguishes I/O failures from format specific problems, so a corrupt or truncated message base is reported rather than silently treated as end of file.

## Example

``` cargo run --example read_jam data/jam/ra ```

## Benchmarks

`cargo bench` measures opening, reading, searching, appending, packing,
reindexing and verifying a 2000 message base, plus header serialization.
Pass a name to run part of it, for example `cargo bench -- read`.

## More formats are awaiting

Maybe Squish next?
