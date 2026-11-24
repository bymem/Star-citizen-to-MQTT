//! Opens a recording from either a zip or a folder.
//!
//! Recordings are small (tens of MB for a long session), so everything is
//! read into memory up front. That keeps replay simple and lets every client
//! share the same frame buffer.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use bytes::Bytes;

use super::files;
use super::format::{FrameReader, FrameRecord};

/// Reads one file of the recording by name; `None` if it is absent.
type FileGetter = dyn Fn(&str) -> anyhow::Result<Option<Vec<u8>>>;

pub struct Recording {
    pub source: PathBuf,
    pub frames_bin: Bytes,
    pub actions_bin: Option<Bytes>,
    pub schema_desc: Option<Bytes>,
    pub meta_json: Option<String>,
    pub log: Option<String>,
    pub schema_proto: Option<String>,
    pub actions_json: Option<String>,
}

impl Recording {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let get: Box<FileGetter> = if path.is_dir() {
            let dir = path.to_path_buf();
            Box::new(move |name| read_optional(&dir.join(name)))
        } else {
            let data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            Box::new(move |name| read_from_zip(&data, name))
        };

        let Some(frames) = get(files::FRAMES)? else {
            bail!("{} has no {}", path.display(), files::FRAMES);
        };
        let text = |name| -> anyhow::Result<Option<String>> {
            Ok(get(name)?.map(|b| String::from_utf8_lossy(&b).into_owned()))
        };

        Ok(Self {
            source: path.to_path_buf(),
            frames_bin: frames.into(),
            actions_bin: get(files::ACTIONS_BIN)?.map(Bytes::from),
            schema_desc: get(files::SCHEMA_DESC)?.map(Bytes::from),
            meta_json: text(files::META)?,
            log: text(files::LOG)?,
            schema_proto: text(files::SCHEMA_PROTO)?,
            actions_json: text(files::ACTIONS_JSON)?,
        })
    }

    pub fn frames(&self) -> anyhow::Result<FrameReader> {
        Ok(FrameReader::new(self.frames_bin.clone())?)
    }

    /// All complete records, plus the number of trailing bytes that did not
    /// form a full record.
    pub fn all_frames(&self) -> anyhow::Result<(Vec<FrameRecord>, usize)> {
        let mut reader = self.frames()?;
        let records: Vec<_> = reader.by_ref().collect();
        Ok((records, reader.truncated_bytes()))
    }
}

fn read_optional(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(data) => Ok(Some(data)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Finds `name` in the zip, either at the root or inside the single
/// top-level folder that `record` creates.
fn read_from_zip(data: &[u8], name: &str) -> anyhow::Result<Option<Vec<u8>>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(data)).context("opening zip")?;
    let index = (0..archive.len()).find(|&i| {
        archive.name_for_index(i).is_some_and(|entry| {
            entry.is_ok_and(|entry| {
                entry == name
                    || entry
                        .rsplit_once('/')
                        .is_some_and(|(dir, file)| file == name && !dir.contains('/'))
            })
        })
    });
    let Some(index) = index else {
        return Ok(None);
    };
    let mut file = archive.by_index(index)?;
    let mut out = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut out)?;
    Ok(Some(out))
}
