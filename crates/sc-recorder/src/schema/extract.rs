//! Finds the telemetry `FileDescriptorProto`s that protobuf embeds in the
//! game executable (Rust port of upstream `sc_extract_proto.py`).
//!
//! protoc-generated C++ keeps each `.proto` file's serialized descriptor as
//! a byte blob. Every blob starts with field 1 (`name`), so we search for
//! that prefix and then walk forward while the bytes still look like a
//! `FileDescriptorProto`, because the blob's end is not stored anywhere.

use std::collections::{BTreeMap, HashSet};

use prost::Message;
use prost_types::{FileDescriptorProto, FileDescriptorSet};
use regex::bytes::Regex;

use crate::sections::read_varint;

/// Only our service's protos; skips the bundled opentelemetry ones.
const NAME_PREFIX: &str = "sc/game/telemetry/";
const MAX_FIELD_LEN: u64 = 5_000_000;

pub const NOT_FOUND_HINT: &str = "No telemetry schema found in the executable. The game may have \
changed how it embeds the schema. Recording still works without it.";

/// Extracts every telemetry descriptor found in `data`, ordered so each
/// file comes after its dependencies.
pub fn extract(data: &[u8]) -> FileDescriptorSet {
    // 0x0A = field 1, wire type 2; then a one-byte length; then the name.
    let candidate = Regex::new(r"(?-u)\x0A[\x01-\x7F]sc/game/telemetry/").unwrap();
    let valid_name = Regex::new(r"(?-u)^[\w./-]+\.proto$").unwrap();

    let mut by_name: BTreeMap<String, (usize, FileDescriptorProto)> = BTreeMap::new();
    for m in candidate.find_iter(data) {
        let start = m.start();
        let name_len = data[start + 1] as usize;
        let Some(name) = data.get(start + 2..start + 2 + name_len) else {
            continue;
        };
        if !name.starts_with(NAME_PREFIX.as_bytes()) || !valid_name.is_match(name) {
            continue;
        }
        let Some((size, file)) = decode_at(data, start) else {
            continue;
        };
        if file.message_type.is_empty() && file.service.is_empty() {
            continue;
        }
        // Same file embedded twice (e.g. debug copy): keep the biggest.
        let entry = by_name.entry(file.name().to_string());
        match entry {
            std::collections::btree_map::Entry::Vacant(v) => {
                v.insert((size, file));
            }
            std::collections::btree_map::Entry::Occupied(mut o) => {
                if size > o.get().0 {
                    o.insert((size, file));
                }
            }
        }
    }

    let files: Vec<_> = by_name.into_values().map(|(_, f)| f).collect();
    warn_missing_dependencies(&files);
    FileDescriptorSet {
        file: dependency_order(files),
    }
}

/// Walks fields from `start` and decodes the longest prefix that is a valid
/// `FileDescriptorProto`. Returns its byte size and the decoded file.
fn decode_at(data: &[u8], start: usize) -> Option<(usize, FileDescriptorProto)> {
    let ends = field_ends(data, start);
    // Trailing bytes can look like valid fields by accident; drop fields
    // from the end until protobuf accepts the blob.
    for &end in ends.iter().rev() {
        if let Ok(file) = FileDescriptorProto::decode(&data[start..end]) {
            return Some((end - start, file));
        }
    }
    None
}

/// Offsets just after each field that passes the structural checks.
fn field_ends(data: &[u8], start: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut pos = start;
    let mut last_field = 0u64;
    while pos < data.len() {
        let Some(key) = read_varint(data, &mut pos) else {
            break;
        };
        let (field, wire_type) = (key >> 3, key & 7);
        let expected = match field {
            1..=9 | 12 => 2,
            10 | 11 | 14 => 0,
            _ => break,
        };
        // Real descriptors are written in field order and have one name.
        if wire_type != expected || field < last_field || (field == 1 && last_field == 1) {
            break;
        }
        last_field = field;
        if wire_type == 0 {
            if read_varint(data, &mut pos).is_none() {
                break;
            }
        } else {
            let Some(len) = read_varint(data, &mut pos) else {
                break;
            };
            if len > MAX_FIELD_LEN || pos + len as usize > data.len() {
                break;
            }
            pos += len as usize;
        }
        ends.push(pos);
    }
    ends
}

fn warn_missing_dependencies(files: &[FileDescriptorProto]) {
    let found: HashSet<&str> = files.iter().map(|f| f.name()).collect();
    for file in files {
        for dep in &file.dependency {
            if !dep.starts_with("google/protobuf/") && !found.contains(dep.as_str()) {
                tracing::warn!(
                    "{} imports {dep}, which was not found in the executable",
                    file.name()
                );
            }
        }
    }
}

/// Puts dependencies before the files that import them, so tools that load
/// files one by one (like protoc) accept the set.
fn dependency_order(files: Vec<FileDescriptorProto>) -> Vec<FileDescriptorProto> {
    let mut remaining: BTreeMap<String, FileDescriptorProto> = files
        .into_iter()
        .map(|f| (f.name().to_string(), f))
        .collect();
    let mut ordered = Vec::new();
    while let Some(name) = remaining.keys().next().cloned() {
        visit(&name, &mut remaining, &mut ordered);
    }
    ordered
}

fn visit(
    name: &str,
    remaining: &mut BTreeMap<String, FileDescriptorProto>,
    ordered: &mut Vec<FileDescriptorProto>,
) {
    // Removing before recursing also stops import cycles.
    let Some(file) = remaining.remove(name) else {
        return;
    };
    for dep in &file.dependency {
        visit(dep, remaining, ordered);
    }
    ordered.push(file);
}
