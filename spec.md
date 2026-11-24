# sc-recorder — Build Spec (Phase 0 of SC MQTT Companion)

Hand-off spec for Claude Code. Build this first. It captures real data from Star Citizen on the gaming PC, so the rest of the companion can be developed on the Ubuntu VPS without the game.

**Status:** Ready to build  
**Dev host:** Ubuntu VPS (headless)  
**Build and release:** GitHub Actions (tag-triggered release, see §10)  
**Run host:** Windows 10/11 gaming PC with Star Citizen  
**Language:** Rust (stable)

## 1. Purpose and workflow

1. Code is written and tested on the VPS.
2. Pushing a version tag (e.g. `v0.1.0`) makes GitHub Actions build Windows and Linux binaries and publish a GitHub Release.
3. The user downloads `sc-recorder.exe` from the Release page, double-clicks it on the gaming PC and plays for 10–15 minutes (on foot, in a ship, quantum, landing, combat), then closes it.
4. It produces one zip file with everything needed.
5. On the VPS, the Linux build decodes the recording and can replay it as a fake game server on `127.0.0.1:2942`, so the companion can be developed against it.

## 2. Hard rules

- **Read-only towards the game and system.** The gRPC client module has no function for `TriggerGameAction` at all; only the `replay` server knows that method (§8). Game files (`StarCitizen.exe`, `Game.log`) are opened read-only with shared access.
- **Writes only its own output folder** (next to the exe, or `--out`). No registry, no AppData, no config files, no edits to `user.cfg`.
- **No network** other than `127.0.0.1:2942`.
- **No personal names** in code comments, README or commit messages. Use "Maintainer" if attribution is needed.
- Code comments in English, explaining why, not just what.

## 3. Known API facts

Source: github.com/Sokren/sc-telemetry-streamdeck, `schema/` folder (checked 2026-11-21, SC 5.0.202.10025 PTU build 13248297). Upstream license is "To be decided", so do not vendor their files into our repo. Fetch them for local dev and testing only.

- Endpoint: `127.0.0.1:2942`, plain HTTP/2 gRPC, no TLS, no auth.
- Package `sc.game.telemetry.v1`, service `GameTelemetryService`.

| Method | Full path | Request | Response |
| --- | --- | --- | --- |
| TelemetryStream | `/sc.game.telemetry.v1.GameTelemetryService/TelemetryStream` | empty `TelemetryStreamRequest` | server stream of `TelemetryStreamResponse` |
| ListTriggerableActions | `/sc.game.telemetry.v1.GameTelemetryService/ListTriggerableActions` | empty request | `ListTriggerableActionsResponse` |
| TriggerGameAction | never called by the recorder |  |  |

- Both requests are empty messages and encode as zero bytes, so recording needs no schema. The recorder stores the raw protobuf bytes of each response.
- `TelemetryStreamResponse` top-level fields, used only for live stats: 3 vehicle, 4 weapons, 5 events, 6 actor, 7 force\_feedback, 8 turret (1 and 2 reserved).
- `ListTriggerableActionsResponse { repeated TriggerableAction actions = 1; }` with `TriggerableAction { string action_name = 1; string category = 2; bool holdable = 3; }`. Hand-write these as `prost` structs.
- About 17 frames per second. Frames are partial: sections and zero values are omitted. The stream only starts after the character has spawned.
- Telemetry must be enabled by the user (`server.game.telemetry.enabled=1` in `user.cfg`). `Game.log` shows `Local services server is running on 127.0.0.1:2942` when it's up.

## 4. CLI

One binary, `sc-recorder`, with subcommands. No arguments means `record`, so double-clicking works.

- `sc-recorder [record] [--minutes N] [--endpoint URL] [--out DIR] [--exe PATH] [--no-schema]`
- `sc-recorder decode <recording.zip|dir> [--desc FILE] [--out frames.jsonl] [--scrub]`
- `sc-recorder replay <recording.zip|dir> [--listen 127.0.0.1:2942] [--speed 1.0] [--loop]`
- `sc-recorder extract-schema <StarCitizen.exe> [--out schema.desc] [--compare other.desc]`

`record` and `extract-schema` are for the gaming PC. `decode` and `replay` are for the VPS, but all subcommands are built for both targets.

## 5. record: behaviour

### 5.1 Startup

1. Print a short banner: what it does, that it is read-only, and "Press Ctrl+C or close this window to stop".
2. Find the game: poll every 2 s for a process named `StarCitizen.exe` (`sysinfo`) and take its exe path. `--exe` overrides this. While waiting, show a single updating line: `Waiting for Star Citizen to start…`.
3. Game version: read `<channel folder>/Game.log` (channel folder = parent of `Bin64`) read-only and grab the `FileVersion:` and `Build(` lines. A missing file is not an error.
4. Schema: run the extractor (§7) on the exe and save `schema.desc` and `schema.proto`. On failure, warn and keep going, since recording works without it. Skipped with `--no-schema`.

### 5.2 Connect

- Connect to `--endpoint` (default `http://127.0.0.1:2942`). Retry every 3 s with the status line `Waiting for telemetry server… (is telemetry enabled in user.cfg?)`.
- After 60 s without a connection, print the `user.cfg` hint once: the exact line to add and where the file lives. Keep retrying. Never edit the file.

### 5.3 Capture

1. Call `ListTriggerableActions` once per connection. Save `actions.bin` (raw response) and `actions.json` (decoded, pretty).
2. Open `TelemetryStream` and append every message to `frames.bin` (§6).
3. Refresh a live status line once per second, e.g. `Recording 04:12 | 4 301 frames | 17.1 fps | actor ✓ vehicle ✓ weapons ✓ events 3 ff ✓ turret –`. Section presence comes from a cheap top-level tag scan of the raw bytes, with no schema.
4. If no frames arrive within 10 s of opening the stream, print `Connected. Waiting for your character to spawn…`.
5. If the stream ends or errors (game closed, server restart, map change), log it to `meta.json`, print it, go back to finding the game and connecting, and keep appending to the same recording. Each connection is a new segment.

### 5.4 Stop and finalise

Stop on Ctrl+C, Ctrl+Break, the console window closing (`tokio::signal::windows::ctrl_close`) or `--minutes` reached. Then:

1. Flush and close `frames.bin`.
2. Write `meta.json` (§6).
3. Zip the folder to `sc-recording-<YYYYMMDD-HHMMSS>.zip` next to it.
4. Print the zip path, its size and a coverage checklist, e.g. `Covered: actor ✓ vehicle ✓ weapons ✓ events ✓ force_feedback ✓ turret ✗`, with a tip for anything not seen.
5. If started by double-click (no args), wait for Enter before exiting. On console close, finish within Windows' \~5 s budget. The zip is the only slow step, and the folder is valid on its own.

### 5.5 Durability

- `frames.bin` goes through a `BufWriter` flushed at least once per second, so a crash loses at most about 1 s. The decoder must tolerate a truncated last record.
- `meta.json` is rewritten atomically (temp file, then rename) on every segment change, not only at the end.

## 6. Output format

### 6.1 Folder layout

| File | Contents |
| --- | --- |
| `frames.bin` | Raw telemetry frames |
| `actions.bin` | Raw ListTriggerableActions response (last connection) |
| `actions.json` | Decoded action list |
| `schema.desc` | FileDescriptorSet extracted from the running exe, if it succeeded |
| `schema.proto` | Readable version of the schema |
| `meta.json` | Recording metadata |
| `recorder.log` | Everything printed to the console, plus warnings |

The folder is named `sc-recording-<YYYYMMDD-HHMMSS>/` and zipped next to itself.

### 6.2 frames.bin

Little-endian, easy to parse from any language.

- Header: 8 bytes, magic `SCREC\0\0\x01` (the last byte is format version 1).
- Each record: `u64 t_us` (microseconds since recording start, monotonic clock), `u16 segment` (connection index from 0), `u32 len`, then `len` bytes of raw `TelemetryStreamResponse` exactly as received.

### 6.3 meta.json

Fields: `format` (1), `recorder_version`, `started_at` and `ended_at` (local ISO 8601), `game` (`exe_path`, `exe_sha256`, `channel` LIVE/PTU/EPTU, `file_version`, `build`), `schema` (`extracted`, `files`, `sha256`), `segments` (per segment: `index`, `connected_at_us`, `first_frame_at_us`, `ended_at_us`, `end_reason`, `frames`), `totals` (`frames`, `bytes`, `sections_seen` count per section) and `actions_count`.

## 7. Schema extraction

A Rust port of upstream `sc_extract_proto.py`, using `prost-types::FileDescriptorProto`.

1. Read the whole exe into memory, read-only with shared access.
2. Find candidates matching the bytes `0x0A <len, one byte 0x01–0x7F> <name>`, where `<name>` matches `[\w./-]+?\.proto`, its length equals `<len>`, and it starts with `sc/game/telemetry/`. This excludes the bundled opentelemetry protos. Use `regex::bytes`.
3. From each candidate, walk forward field by field while it still looks like a `FileDescriptorProto`. Expected wire types: fields 1–9 and 12 are length-delimited (2), fields 10, 11 and 14 are varints (0). Anything else ends the walk. Field numbers must not decrease, field 1 must not repeat, and lengths over 5 000 000 or past the buffer are rejected. Record the end offset after each accepted field.
4. Decode `data[start..end]` at the last end offset. On failure, drop the last field and retry until it decodes or nothing is left.
5. Keep results with at least one message or service. If a file name appears more than once, keep the largest.
6. Write `schema.desc` (a `FileDescriptorSet`) and a readable `schema.proto` covering enums, nested messages, proto3 `optional`, `repeated`, reserved ranges, oneofs and services.
7. Warn about dependencies that weren't found, ignoring `google/protobuf/*`.
8. `--compare other.desc` lists fields `(message, number) → name` present in one set and not the other. This is how schema changes between patches are spotted.

If nothing is found, say clearly that the game may have changed how it embeds the schema, and that recording still works.

## 8. decode and replay (VPS side)

### decode

- Input is a zip or a folder. Uses the recording's `schema.desc`, or `--desc`.
- Output is JSONL, one line per frame: `t_us`, `segment` and `frame`, decoded with `prost-reflect` using proto field names.
- Prints a summary of every field path seen: how often, min and max for numbers, distinct values for enums and strings. This feeds the companion's default sensor picks.
- `--scrub` writes a cleaned copy of the recording (see §11).

### replay

- A tonic gRPC server on `--listen` (default `127.0.0.1:2942`) with the same method paths and a raw bytes codec.
- `TelemetryStream` sends the recorded payloads, sleeping `Δt_us / speed` between them. `--loop` restarts at the end.
- `ListTriggerableActions` returns `actions.bin` as is.
- `TriggerGameAction` exists only here. It logs `action_name` and `hold_ms` (hand-written prost struct) and returns status OK (1). This is how the companion's buttons get tested on the VPS.
- Several clients at once must work, each with its own replay cursor.

## 9. Implementation notes

| Need | Crate |
| --- | --- |
| Async | `tokio` (rt-multi-thread, macros, signal, time, fs) |
| gRPC client and server | `tonic` (no TLS features) with a custom raw-bytes `Codec` |
| Protobuf | `prost`, `prost-types`, `prost-reflect` |
| CLI | `clap` (derive) |
| Process lookup | `sysinfo` |
| Byte regex | `regex` |
| Zip | `zip` with default features off and `deflate` on |
| Hash | `sha2` |
| JSON | `serde`, `serde_json` |
| Time | `chrono` for names and metadata, `std::time::Instant` for `t_us` |
| Logging | `tracing`, `tracing-subscriber` (console and file) |

- Raw codec: implement `tonic::codec::Codec` with `bytes::Bytes` in and out. Call through `tonic::client::Grpc` with `server_streaming` and `unary` and `PathAndQuery::from_static`, calling `ready().await` first.
- Windows: enable VT processing for the updating status line, falling back to plain lines every 5 s. No CLI args means interactive mode (wait for Enter at the end). Windows-only code sits behind `#[cfg(windows)]` so everything also builds on Linux.
- Release profile: `strip = true`, `lto = true`, `opt-level = "z"`, `codegen-units = 1`. Target size well under 15 MB.
- On the VPS, use plain `cargo build` and `cargo test` for Linux. No local Windows cross-compiling; GitHub Actions makes the exe.

## 10. Build and release with GitHub Actions

All builds that leave the VPS come from GitHub Actions. Pushing a version tag builds every target and publishes a full GitHub Release.

### ci.yml (every push and pull request)

- Runs on `ubuntu-latest` and `windows-latest`.
- Steps: checkout, `dtolnay/rust-toolchain@stable` with `clippy` and `rustfmt`, `Swatinem/rust-cache`, then `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`.
- The Linux job also runs the round-trip test (§12).
- Uploads the Windows `sc-recorder.exe` as a build artifact, so test builds can be grabbed without making a release.

### release.yml (on tags matching `v*`)

1. **Build job**, a matrix of two:
   - `windows-latest`, target `x86_64-pc-windows-msvc`, output `sc-recorder.exe`.
   - `ubuntu-latest`, target `x86_64-unknown-linux-gnu`, output `sc-recorder`.
2. Each job runs `cargo build --release --locked --target <target>`, then packages:
   - Windows: `sc-recorder-<version>-windows-x64.zip` with the exe and a short `README.txt` (how to enable telemetry and how to run it).
   - Linux: `sc-recorder-<version>-linux-x64.tar.gz`.
3. Each job writes a SHA-256 file per package and uploads them as artifacts.
4. **Release job** (needs both builds) downloads all artifacts, merges the checksums into `SHA256SUMS.txt`, and creates the GitHub Release with `softprops/action-gh-release`: name = the tag, notes generated from commits, all packages and `SHA256SUMS.txt` attached.
5. The tag must match the `version` in `Cargo.toml`. The workflow fails early if it doesn't.
6. Tags with a suffix such as `v0.2.0-rc.1` are published as pre-releases.

### Notes

- The workflow only needs `permissions: contents: write`. No extra secrets.
- Code signing is left out for now. Add it later as a release step, since SmartScreen will warn on an unsigned exe.
- The release flow: bump the version in `Cargo.toml`, commit, tag `vX.Y.Z`, push the tag, then download the zip from the Releases page on the gaming PC.

## 11. Privacy

Recordings can contain personal data: `actor.state.uec_balance`, `seat_name`, vehicle names, and the Windows username in `exe_path`.

- `decode --scrub` zeroes `uec_balance`, replaces the user folder in paths with `<user>`, and writes a scrubbed copy of the recording, re-encoding frames with `prost-reflect`.
- Keep recordings and extracted schemas in a private repo or outside git. The schema comes from the game binary, so don't publish it. Add `*.zip`, `sc-recording-*/` and `*.desc` to `.gitignore`.

## 12. Testing on the VPS (no game)

1. A test helper builds synthetic `TelemetryStreamResponse` frames with `prost-reflect` from upstream's `sc_extrait.desc`, fetched locally and not committed, and writes a `frames.bin`.
2. `replay` that recording on `127.0.0.1:2942`.
3. Run `record --minutes 1 --no-schema --exe <any file>` against it.
4. `decode` the result and compare with the input. The payloads must be byte-identical.
5. Kill `record` mid-run with `kill -9`, and check `decode` still reads everything up to the last full record.
6. Run `extract-schema` on a test binary with the upstream `.desc` files embedded at random offsets among junk bytes. The output must match.

## 13. What to send back from the gaming PC

The single `sc-recording-<timestamp>.zip`. Ideally two sessions:

1. **LIVE** channel, normal play: on foot, then ship, quantum, landing, and combat if possible. Sit in a turret if you want turret data.
2. **PTU** channel if available, to compare schema versions with `extract-schema --compare`.

## 14. Acceptance criteria

- [ ] Pushing a `v*` tag produces a GitHub Release with the Windows zip, the Linux tarball and `SHA256SUMS.txt`.
- [ ] Double-clicking `sc-recorder.exe` with the game running produces a zip with no further input apart from Enter at the end.
- [ ] Starting it before the game works: it waits, then records.
- [ ] A game restart mid-recording produces two segments in one recording.
- [ ] Closing the console window still leaves a valid `frames.bin` and `meta.json`.
- [ ] The client module used by `record` has no `TriggerGameAction` call. Only `replay` handles it, as a server.
- [ ] Nothing is written outside the output folder.
- [ ] `decode` and `replay` on Ubuntu work with a zip recorded on Windows.
- [ ] The round-trip test passes in CI.
