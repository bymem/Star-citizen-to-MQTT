//! Diffs two descriptor sets by `(message, field number) -> field name`, to
//! spot schema changes between game patches.

use std::collections::BTreeMap;
use std::fmt::Write;

use prost_types::{DescriptorProto, FileDescriptorSet};

type FieldMap = BTreeMap<(String, i32), String>;

pub fn compare(
    a: &FileDescriptorSet,
    b: &FileDescriptorSet,
    a_label: &str,
    b_label: &str,
) -> String {
    let (fa, fb) = (fields(a), fields(b));
    let mut out = String::new();
    let mut section = |title: &str, lines: Vec<String>| {
        let _ = writeln!(out, "{title}: {}", lines.len());
        for line in lines {
            let _ = writeln!(out, "  {line}");
        }
    };

    let only = |x: &FieldMap, y: &FieldMap| -> Vec<String> {
        x.iter()
            .filter(|(k, _)| !y.contains_key(*k))
            .map(|((msg, num), name)| format!("{msg} #{num} -> {name}"))
            .collect()
    };
    section(&format!("Only in {a_label}"), only(&fa, &fb));
    section(&format!("Only in {b_label}"), only(&fb, &fa));
    section(
        "Renamed",
        fa.iter()
            .filter_map(|(k, name)| {
                let other = fb.get(k)?;
                (other != name).then(|| format!("{} #{}: {name} -> {other}", k.0, k.1))
            })
            .collect(),
    );
    out
}

fn fields(set: &FileDescriptorSet) -> FieldMap {
    let mut map = FieldMap::new();
    for file in &set.file {
        let prefix = file.package().to_string();
        for msg in &file.message_type {
            collect(&prefix, msg, &mut map);
        }
    }
    map
}

fn collect(prefix: &str, msg: &DescriptorProto, map: &mut FieldMap) {
    let full = if prefix.is_empty() {
        msg.name().to_string()
    } else {
        format!("{prefix}.{}", msg.name())
    };
    for field in &msg.field {
        map.insert((full.clone(), field.number()), field.name().to_string());
    }
    for nested in &msg.nested_type {
        collect(&full, nested, map);
    }
}
