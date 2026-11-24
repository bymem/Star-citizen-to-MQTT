//! On-disk recording format (spec §6) and helpers to read and write it.

pub mod archive;
pub mod format;
pub mod meta;
pub mod source;

pub use format::{FrameReader, FrameRecord, FrameWriter};
pub use meta::Meta;
pub use source::Recording;

/// File names inside a recording folder or zip.
pub mod files {
    pub const FRAMES: &str = "frames.bin";
    pub const ACTIONS_BIN: &str = "actions.bin";
    pub const ACTIONS_JSON: &str = "actions.json";
    pub const SCHEMA_DESC: &str = "schema.desc";
    pub const SCHEMA_PROTO: &str = "schema.proto";
    pub const META: &str = "meta.json";
    pub const LOG: &str = "recorder.log";
}
