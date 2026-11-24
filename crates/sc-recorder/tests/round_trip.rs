//! End-to-end test on the dev machine (spec §12): replay a synthetic
//! recording, record it with the real binary, and check the frames come
//! back byte-identical. Unix only, matching the CI plan (spec §10).
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use bytes::Bytes;
use common::*;
use sc_recorder::recording::{Meta, Recording};

const FRAMES: usize = 200;
const STEP_US: u64 = 10_000;

fn start_replay(input: &Path, port: u16, extra: &[&str]) -> KillOnDrop {
    let child = Command::new(BIN)
        .arg("replay")
        .arg(input)
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(extra)
        .stdout(Stdio::null())
        .spawn()
        .expect("starting replay");
    KillOnDrop(child)
}

fn record_cmd(port: u16, out: &Path, exe: &Path, minutes: &str) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.arg("record")
        .args(["--minutes", minutes, "--no-schema"])
        .args(["--endpoint", &format!("http://127.0.0.1:{port}")])
        .arg("--out")
        .arg(out)
        .arg("--exe")
        .arg(exe)
        .stdout(Stdio::null());
    cmd
}

fn find_in(dir: &Path, pred: impl Fn(&Path) -> bool) -> PathBuf {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| pred(p))
        .unwrap_or_else(|| panic!("nothing matching in {}", dir.display()))
}

/// Every recorded segment must be the input stream from its start, in
/// order and byte-identical. Returns the number of records.
fn assert_matches_input(rec: &Recording, input: &[Bytes]) -> usize {
    let (records, _) = rec.all_frames().unwrap();
    assert!(!records.is_empty(), "no frames recorded");
    let mut expected = 0;
    let mut segment = records[0].segment;
    for r in &records {
        if r.segment != segment {
            segment = r.segment;
            expected = 0;
        }
        // --loop replays wrap around.
        assert_eq!(r.payload, input[expected % input.len()], "payload mismatch");
        expected += 1;
    }
    records.len()
}

fn decode(input: &Path, desc: &Path, out: &Path, extra: &[&str]) -> String {
    let output = Command::new(BIN)
        .arg("decode")
        .arg(input)
        .arg("--desc")
        .arg(desc)
        .arg("--out")
        .arg(out)
        .args(extra)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "decode failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn replay_record_decode_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let desc = test_schema_bytes();
    let input = synthetic_frames(&desc, FRAMES);
    let input_dir = tmp.path().join("input");
    write_recording(&input_dir, &desc, &input, STEP_US);
    let desc_path = input_dir.join("schema.desc");

    let port = free_port();
    let _replay = start_replay(&input_dir, port, &["--speed", "2"]);
    let out = tmp.path().join("out");
    std::fs::create_dir_all(&out).unwrap();

    // 0.15 min = 9 s: enough for the first segment (1 s of replay) even if
    // the first connect attempt races the server start.
    let status = record_cmd(port, &out, &desc_path, "0.15").status().unwrap();
    assert!(status.success());

    let zip = find_in(&out, |p| p.extension().is_some_and(|e| e == "zip"));
    let rec = Recording::open(&zip).unwrap();
    let count = assert_matches_input(&rec, &input);
    let (records, _) = rec.all_frames().unwrap();
    assert!(
        records.iter().filter(|r| r.segment == 0).count() == FRAMES,
        "first segment should hold the whole replay"
    );

    // The replay ends its stream, so record must reconnect as a new segment.
    let meta: Meta = serde_json::from_str(rec.meta_json.as_deref().unwrap()).unwrap();
    assert!(meta.segments.len() >= 2, "expected a reconnect segment");
    assert_eq!(meta.totals.frames as usize, count);
    assert_eq!(meta.actions_count, Some(2));
    assert!(meta.ended_at.is_some());

    // Decode the zip and check every frame became a JSON line.
    let jsonl = tmp.path().join("frames.jsonl");
    let summary = decode(&zip, &desc_path, &jsonl, &[]);
    let lines = std::fs::read_to_string(&jsonl).unwrap();
    assert_eq!(lines.lines().count(), count);
    assert!(summary.contains("Fields seen"));

    // --scrub writes a cleaned copy and decodes that copy.
    let scrubbed_jsonl = tmp.path().join("scrubbed.jsonl");
    decode(&zip, &desc_path, &scrubbed_jsonl, &["--scrub"]);
    let scrubbed_zip = zip.with_file_name(format!(
        "{}-scrubbed.zip",
        zip.file_stem().unwrap().to_string_lossy()
    ));
    let scrubbed = Recording::open(&scrubbed_zip).unwrap();
    assert_eq!(scrubbed.all_frames().unwrap().0.len(), count);
    if std::env::var_os("SC_UPSTREAM_DESC").is_none() {
        assert!(
            lines.contains("\"uec_balance\""),
            "fake frames carry balances"
        );
        let text = std::fs::read_to_string(scrubbed_jsonl).unwrap();
        assert!(
            !text.contains("\"uec_balance\""),
            "balance should be zeroed"
        );
    }
}

#[test]
fn killed_recording_is_still_readable() {
    let tmp = tempfile::tempdir().unwrap();
    let desc = test_schema_bytes();
    let input = synthetic_frames(&desc, FRAMES);
    let input_dir = tmp.path().join("input");
    write_recording(&input_dir, &desc, &input, STEP_US);
    let desc_path = input_dir.join("schema.desc");

    let port = free_port();
    let _replay = start_replay(&input_dir, port, &["--loop"]);
    let out = tmp.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let mut record = KillOnDrop(record_cmd(port, &out, &desc_path, "1").spawn().unwrap());

    // Wait until frames have been flushed at least once, then SIGKILL.
    let deadline = Instant::now() + Duration::from_secs(20);
    let frames_bin = loop {
        assert!(Instant::now() < deadline, "record never flushed frames");
        if let Ok(Some(Ok(entry))) = std::fs::read_dir(&out).map(|mut d| d.next()) {
            let f = entry.path().join("frames.bin");
            if std::fs::metadata(&f).is_ok_and(|m| m.len() > 1_000) {
                break f;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    std::thread::sleep(Duration::from_millis(1500));
    record.0.kill().unwrap(); // SIGKILL: no cleanup runs.
    record.0.wait().unwrap();

    let folder = frames_bin.parent().unwrap();
    let rec = Recording::open(folder).unwrap();
    assert_matches_input(&rec, &input);
    // meta.json is rewritten atomically, so it must always parse.
    let _: Meta = serde_json::from_str(rec.meta_json.as_deref().unwrap()).unwrap();

    let jsonl = tmp.path().join("frames.jsonl");
    decode(folder, &desc_path, &jsonl, &[]);
}
