//! `meta.json`: recording metadata, rewritten atomically on every change.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Meta {
    pub format: u32,
    pub recorder_version: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub game: GameInfo,
    pub schema: SchemaInfo,
    pub segments: Vec<Segment>,
    pub totals: Totals,
    pub actions_count: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GameInfo {
    pub exe_path: Option<String>,
    pub exe_sha256: Option<String>,
    /// Channel folder name, e.g. LIVE, PTU, EPTU.
    pub channel: Option<String>,
    pub file_version: Option<String>,
    pub build: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchemaInfo {
    pub extracted: bool,
    pub files: Vec<String>,
    /// SHA-256 of `schema.desc`.
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Segment {
    pub index: u16,
    pub connected_at_us: u64,
    pub first_frame_at_us: Option<u64>,
    pub ended_at_us: Option<u64>,
    pub end_reason: Option<String>,
    pub frames: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Totals {
    pub frames: u64,
    /// Payload bytes, excluding record headers.
    pub bytes: u64,
    /// Number of frames that contained each top-level section.
    pub sections_seen: BTreeMap<String, u64>,
}

impl Meta {
    /// Writes to a temp file and renames over the target, so a crash or a
    /// closed console never leaves a half-written `meta.json`.
    pub fn write_atomic(&self, path: &Path) -> io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)
    }
}
