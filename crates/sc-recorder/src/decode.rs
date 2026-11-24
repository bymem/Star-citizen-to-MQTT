//! `decode` subcommand: frames to JSONL, a field summary, and `--scrub`.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufWriter, Cursor, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;
use bytes::Bytes;
use prost::Message;
use prost_reflect::{
    DescriptorPool, DynamicMessage, Kind, MessageDescriptor, SerializeOptions, Value,
};
use regex::Regex;
use serde::{Serialize, Serializer};
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::recording::{FrameRecord, FrameWriter, Recording, files};
use crate::{TELEMETRY_RESPONSE, schema};

pub struct DecodeArgs {
    pub input: PathBuf,
    pub desc: Option<PathBuf>,
    pub out: PathBuf,
    pub scrub: bool,
}

pub fn run(args: DecodeArgs) -> anyhow::Result<()> {
    let rec = Recording::open(&args.input)?;
    let desc = match &args.desc {
        Some(path) => {
            Bytes::from(fs::read(path).with_context(|| format!("reading {}", path.display()))?)
        }
        None => rec
            .schema_desc
            .clone()
            .context("recording has no schema.desc; pass one with --desc FILE")?,
    };
    let pool = schema::load_pool(&desc)?;
    let response = find_response(&pool)?;

    let (mut records, truncated) = rec.all_frames()?;
    if truncated > 0 {
        tracing::warn!(
            "Ignored {truncated} bytes of an incomplete last record (recording was cut off)."
        );
    }

    if args.scrub {
        let changed = scrub_frames(&mut records, &response)?;
        let path = scrubbed_path(&args.input);
        write_scrubbed(&rec, &records, &desc, &path)?;
        tracing::info!(
            "Wrote scrubbed copy {} ({changed} frames changed).",
            path.display()
        );
    }

    let mut summary = Summary::default();
    let mut errors = 0u64;
    let mut out = BufWriter::new(
        File::create(&args.out).with_context(|| format!("creating {}", args.out.display()))?,
    );
    let options = SerializeOptions::new()
        .use_proto_field_name(true)
        // Frames are partial; only fields the game sent are interesting.
        .skip_default_fields(true)
        .stringify_64_bit_integers(false);
    for record in &records {
        match DynamicMessage::decode(response.clone(), record.payload.as_ref()) {
            Ok(frame) => {
                summary.add_message("", &frame);
                let line = Line {
                    t_us: record.t_us,
                    segment: record.segment,
                    frame: FrameJson(&frame, &options),
                };
                serde_json::to_writer(&mut out, &line)?;
            }
            Err(e) => {
                errors += 1;
                serde_json::to_writer(
                    &mut out,
                    &serde_json::json!({"t_us": record.t_us, "segment": record.segment, "error": e.to_string()}),
                )?;
            }
        }
        out.write_all(b"\n")?;
    }
    out.flush()?;

    let segments: std::collections::BTreeSet<u16> = records.iter().map(|r| r.segment).collect();
    let duration = records.last().map_or(0, |r| r.t_us) as f64 / 1e6;
    println!(
        "{} frames, {} segment(s), {:.0} s, {} decode errors -> {}",
        records.len(),
        segments.len(),
        duration,
        errors,
        args.out.display()
    );
    print!("{}", summary.render());
    Ok(())
}

/// Finds `TelemetryStreamResponse`, tolerating a package rename.
pub fn find_response(pool: &DescriptorPool) -> anyhow::Result<MessageDescriptor> {
    pool.get_message_by_name(TELEMETRY_RESPONSE)
        .or_else(|| {
            pool.all_messages()
                .find(|m| m.name() == "TelemetryStreamResponse")
        })
        .context("schema has no TelemetryStreamResponse message")
}

/// One JSONL line.
#[derive(Serialize)]
struct Line<'a> {
    t_us: u64,
    segment: u16,
    frame: FrameJson<'a>,
}

struct FrameJson<'a>(&'a DynamicMessage, &'a SerializeOptions);

impl Serialize for FrameJson<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize_with_options(s, self.1)
    }
}

// ---------------------------------------------------------------------------
// Field summary

/// Up to this many distinct values are listed per enum/string/bool field.
const MAX_DISTINCT: usize = 20;

#[derive(Default)]
struct FieldStats {
    count: u64,
    min: Option<f64>,
    max: Option<f64>,
    distinct: BTreeMap<String, u64>,
    more_distinct: bool,
}

impl FieldStats {
    fn number(&mut self, v: f64) {
        self.min = Some(self.min.map_or(v, |m| m.min(v)));
        self.max = Some(self.max.map_or(v, |m| m.max(v)));
    }

    fn value(&mut self, v: String) {
        if let Some(n) = self.distinct.get_mut(&v) {
            *n += 1;
        } else if self.distinct.len() < MAX_DISTINCT {
            self.distinct.insert(v, 1);
        } else {
            self.more_distinct = true;
        }
    }
}

/// Every field path seen across all frames, e.g. `vehicle.speed` or
/// `events[].kind`, with counts and ranges.
#[derive(Default)]
struct Summary {
    fields: BTreeMap<String, FieldStats>,
}

impl Summary {
    fn add_message(&mut self, prefix: &str, msg: &DynamicMessage) {
        for (field, value) in msg.fields() {
            let path = if prefix.is_empty() {
                field.name().to_string()
            } else {
                format!("{prefix}.{}", field.name())
            };
            match value {
                Value::List(items) => {
                    let path = format!("{path}[]");
                    for item in items {
                        self.add_value(&path, &field.kind(), item);
                    }
                }
                Value::Map(entries) => {
                    let path = format!("{path}{{}}");
                    let value_kind = match field.kind() {
                        Kind::Message(entry) => entry.map_entry_value_field().kind(),
                        other => other,
                    };
                    for item in entries.values() {
                        self.add_value(&path, &value_kind, item);
                    }
                }
                other => self.add_value(&path, &field.kind(), other),
            }
        }
    }

    fn add_value(&mut self, path: &str, kind: &Kind, value: &Value) {
        let stats = self.fields.entry(path.to_string()).or_default();
        stats.count += 1;
        match value {
            Value::Bool(b) => stats.value(b.to_string()),
            Value::I32(n) => stats.number(f64::from(*n)),
            Value::I64(n) => stats.number(*n as f64),
            Value::U32(n) => stats.number(f64::from(*n)),
            Value::U64(n) => stats.number(*n as f64),
            Value::F32(n) => stats.number(f64::from(*n)),
            Value::F64(n) => stats.number(*n),
            Value::String(s) => stats.value(s.chars().take(60).collect()),
            Value::Bytes(_) => {}
            Value::EnumNumber(n) => {
                let name = match kind {
                    Kind::Enum(e) => e.get_value(*n).map(|v| v.name().to_string()),
                    _ => None,
                };
                stats.value(name.unwrap_or_else(|| n.to_string()));
            }
            Value::Message(m) => self.add_message(path, m),
            Value::List(_) | Value::Map(_) => {}
        }
    }

    fn render(&self) -> String {
        let width = self.fields.keys().map(String::len).max().unwrap_or(0);
        let mut out = String::from("\nFields seen:\n");
        for (path, s) in &self.fields {
            let mut line = format!("  {path:<width$}  {:>8}x", s.count);
            if let (Some(min), Some(max)) = (s.min, s.max) {
                line.push_str(&format!("  {min} .. {max}"));
            }
            if !s.distinct.is_empty() {
                let values: Vec<String> = s
                    .distinct
                    .iter()
                    .map(|(v, n)| format!("{v:?} ({n})"))
                    .collect();
                line.push_str(&format!("  {}", values.join(", ")));
                if s.more_distinct {
                    line.push_str(", …");
                }
            }
            out.push_str(&line);
            out.push('\n');
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Scrubbing (spec §11)

const SCRUB_FIELDS: &[&str] = &["uec_balance"];

/// Zeroes sensitive fields. Only frames that actually contain one are
/// re-encoded; all others keep their original bytes.
fn scrub_frames(records: &mut [FrameRecord], response: &MessageDescriptor) -> anyhow::Result<u64> {
    let mut changed = 0;
    for record in records.iter_mut() {
        let Ok(mut msg) = DynamicMessage::decode(response.clone(), record.payload.as_ref()) else {
            continue;
        };
        if scrub_message(&mut msg) {
            record.payload = Bytes::from(msg.encode_to_vec());
            changed += 1;
        }
    }
    Ok(changed)
}

fn scrub_message(msg: &mut DynamicMessage) -> bool {
    let mut changed = false;
    let to_clear: Vec<_> = msg
        .fields()
        .filter(|(f, _)| SCRUB_FIELDS.contains(&f.name()))
        .map(|(f, _)| f)
        .collect();
    for field in to_clear {
        // proto3: cleared == zero.
        msg.clear_field(&field);
        changed = true;
    }
    for (_, value) in msg.fields_mut() {
        match value {
            Value::Message(m) => changed |= scrub_message(m),
            Value::List(items) => {
                for item in items {
                    if let Value::Message(m) = item {
                        changed |= scrub_message(m);
                    }
                }
            }
            Value::Map(entries) => {
                for item in entries.values_mut() {
                    if let Value::Message(m) = item {
                        changed |= scrub_message(m);
                    }
                }
            }
            _ => {}
        }
    }
    changed
}

/// Replaces the user folder in Windows and Linux paths with `<user>`.
/// Handles JSON-escaped backslashes too.
pub fn scrub_paths(text: &str) -> String {
    let windows = Regex::new(r#"(?i)([a-z]:(?:\\\\|\\|/)Users(?:\\\\|\\|/))[^\\/"\r\n]+"#).unwrap();
    let linux = Regex::new(r#"(/home/)[^/"\s]+"#).unwrap();
    let text = windows.replace_all(text, "${1}<user>");
    linux.replace_all(&text, "${1}<user>").into_owned()
}

fn scrubbed_path(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording".into());
    input.with_file_name(format!("{stem}-scrubbed.zip"))
}

fn write_scrubbed(
    rec: &Recording,
    records: &[FrameRecord],
    desc: &[u8],
    path: &Path,
) -> anyhow::Result<()> {
    let dir = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording-scrubbed".into());
    let mut frames = FrameWriter::new(Vec::new())?;
    for r in records {
        frames.write(r.t_us, r.segment, &r.payload)?;
    }

    let mut entries: Vec<(&str, Vec<u8>)> = vec![
        (files::FRAMES, frames.into_inner()?),
        (files::SCHEMA_DESC, desc.to_vec()),
    ];
    if let Some(b) = &rec.actions_bin {
        entries.push((files::ACTIONS_BIN, b.to_vec()));
    }
    for (name, text) in [
        (files::ACTIONS_JSON, &rec.actions_json),
        (files::SCHEMA_PROTO, &rec.schema_proto),
        (files::META, &rec.meta_json),
        (files::LOG, &rec.log),
    ] {
        if let Some(t) = text {
            entries.push((name, scrub_paths(t).into_bytes()));
        }
    }

    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .large_file(true);
    for (name, data) in entries {
        zip.start_file(format!("{dir}/{name}"), options)?;
        zip.write_all(&data)?;
    }
    let data = zip.finish()?.into_inner();
    fs::write(path, data).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::scrub_paths;

    #[test]
    fn scrubs_user_folders() {
        assert_eq!(
            scrub_paths(r#"{"exe_path": "C:\\Users\\Someone\\Games\\StarCitizen.exe"}"#),
            r#"{"exe_path": "C:\\Users\\<user>\\Games\\StarCitizen.exe"}"#
        );
        assert_eq!(
            scrub_paths(r"Game: D:\Users\abc\SC.exe"),
            r"Game: D:\Users\<user>\SC.exe"
        );
        assert_eq!(scrub_paths("/home/abc/rec"), "/home/<user>/rec");
        assert_eq!(
            scrub_paths(r"C:\Program Files\RSI"),
            r"C:\Program Files\RSI"
        );
    }
}
