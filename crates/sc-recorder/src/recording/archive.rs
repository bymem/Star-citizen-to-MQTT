//! Zips a recording folder into `<folder>.zip` next to it.

use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

/// Creates `<dir>.zip` containing `<dir_name>/<file>` for each file in `dir`.
/// Returns the zip path. Subfolders are not expected and are skipped.
pub fn zip_dir(dir: &Path) -> io::Result<PathBuf> {
    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad folder name"))?
        .to_string();
    let zip_path = dir.with_extension("zip");
    let tmp_path = dir.with_extension("zip.tmp");

    let mut entries: Vec<_> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let mut zip = ZipWriter::new(BufWriter::new(File::create(&tmp_path)?));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .large_file(true);
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Skip leftovers from an interrupted atomic write.
        if name.ends_with(".tmp") {
            continue;
        }
        zip.start_file(format!("{dir_name}/{name}"), options)
            .map_err(io::Error::other)?;
        io::copy(&mut File::open(entry.path())?, &mut zip)?;
    }
    zip.finish().map_err(io::Error::other)?;
    // Rename at the end so a half-written zip never has the final name.
    fs::rename(&tmp_path, &zip_path)?;
    Ok(zip_path)
}
