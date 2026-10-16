# Changelog

## Unreleased

### Fixed

- PCBoard `lock()` and `unlock()` no longer overwrite the BASIC real message counters in the base header; only the lock status is written.
- `PCBoardMessageBase::read_index` reports a truncated or unreadable index instead of silently returning the records before the damage.
- PCBoard to JAM conversion keeps `ATTACH` extended headers as JAM file attachment subfields.
- QWK to JAM conversion sets `reply_next`, so every reply to a message is reachable through the reply chain.

## 0.3.0 - 2026-08-18

### Breaking

- Replaced boxed errors with typed `jamjam::Error` and format-specific errors.
- Renamed public APIs for consistent Rust naming, including `QwkMessage`, `JhrHeaderInfo`, `read_message_text`, and reply fields.
- Removed the message-number argument from `JamMessage::new`; message bases now allocate numbers.
- Moved physical JAM traversal and direct header mutation under `jam::raw`.
- Added `ControlDat::extra_lines` for optional QWK trailing data.
- Marked the error enums, `Problem`, the verify, repair and pack reports and `PackOptions` as `#[non_exhaustive]`, so later additions no longer break callers. Build pack criteria with `PackOptions::default().with_renumber_from(500)`.
- Removed the unused `MessageType` enums from the PCBoard and QWK modules.

### Added

- Shared and exclusive cross-process JAM locks with transaction helpers.
- Crash-safe packing, interrupted-pack recovery, index rebuilding, and last-read remapping.
- Read-only `verify()` and explicit `repair()` consistency APIs.
- Full-message iteration, recipient and MSGID search helpers, and bounded serialization.
- Criterion benchmarks for reading, writing, searching, packing, verification, and header encoding.
- Seeded mutation fuzzing in the test suite and cargo-fuzz targets for the JAM, PCBoard and QWK parsers.

### Fixed

- Corrected JAM active-message counting, numbering, index lookup, packing, rollback, and stale-handle behavior.
- Rejected truncated or corrupt JAM, PCBoard, and QWK data instead of panicking or silently truncating it.
- Preserved QWK text and extended fields during conversion and completed PCBoard reply chains.
- Made last-read updates atomic and safe for missing, truncated, packed, and renumbered bases.
- Dropped blank lines at the end of `CONTROL.DAT`, which a write could not preserve anyway.

### Performance

- Reused index, header, and text readers during message walks, improving full JAM scans by about 8-9x in the included benchmark.
