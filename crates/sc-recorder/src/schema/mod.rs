//! Schema extraction from the game binary, `.proto` rendering and diffing.

pub mod compare;
pub mod extract;
pub mod render;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use prost::Message;
use prost_reflect::DescriptorPool;
use prost_types::FileDescriptorSet;

pub use extract::extract;

/// Builds a descriptor pool from a `FileDescriptorSet`. Starts from the
/// global pool so `google/protobuf/*` imports resolve even though the
/// extractor never includes them.
pub fn load_pool(desc: &[u8]) -> anyhow::Result<DescriptorPool> {
    let mut pool = DescriptorPool::global();
    pool.decode_file_descriptor_set(desc)
        .context("schema descriptor set is invalid")?;
    Ok(pool)
}

/// Lowercase hex SHA-256.
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `extract-schema` subcommand.
pub fn run_extract(exe: &Path, out: &Path, compare_with: Option<&Path>) -> anyhow::Result<()> {
    let data = fs::read(exe).with_context(|| format!("reading {}", exe.display()))?;
    tracing::info!(
        "Scanning {} ({} MB)…",
        exe.display(),
        data.len() / 1_000_000
    );
    let set = extract(&data);
    if set.file.is_empty() {
        anyhow::bail!("{}", extract::NOT_FOUND_HINT);
    }
    for file in &set.file {
        tracing::info!("Found {}", file.name());
    }
    let (desc_path, proto_path) = write_schema(&set, out)?;
    tracing::info!("Wrote {} and {}", desc_path.display(), proto_path.display());

    if let Some(other) = compare_with {
        let other_set = FileDescriptorSet::decode(
            fs::read(other)
                .with_context(|| format!("reading {}", other.display()))?
                .as_slice(),
        )
        .context("compare file is not a FileDescriptorSet")?;
        print!(
            "{}",
            compare::compare(
                &set,
                &other_set,
                &out.display().to_string(),
                &other.display().to_string()
            )
        );
    }
    Ok(())
}

/// Writes `<out>` (descriptor set) and the same path with `.proto`.
pub fn write_schema(set: &FileDescriptorSet, out: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let proto_path = out.with_extension("proto");
    fs::write(out, set.encode_to_vec()).with_context(|| format!("writing {}", out.display()))?;
    fs::write(&proto_path, render::render(set))
        .with_context(|| format!("writing {}", proto_path.display()))?;
    Ok((out.to_path_buf(), proto_path))
}
