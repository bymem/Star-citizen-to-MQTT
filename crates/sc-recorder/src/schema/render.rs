//! Renders a `FileDescriptorSet` as readable `.proto` text.
//!
//! Meant for humans reading `schema.proto`, not for feeding back into
//! protoc: options and comments are not in the embedded descriptors, so
//! they are left out.

use std::fmt::Write;

use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{
    DescriptorProto, EnumDescriptorProto, FieldDescriptorProto, FileDescriptorProto,
    FileDescriptorSet, ServiceDescriptorProto,
};

pub fn render(set: &FileDescriptorSet) -> String {
    let mut out = String::new();
    for (i, file) in set.file.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        render_file(&mut out, file);
    }
    out
}

fn render_file(out: &mut String, file: &FileDescriptorProto) {
    let _ = writeln!(out, "// ===== {} =====", file.name());
    let syntax = if file.syntax().is_empty() {
        "proto2"
    } else {
        file.syntax()
    };
    let _ = writeln!(out, "syntax = \"{syntax}\";");
    if !file.package().is_empty() {
        let _ = writeln!(out, "package {};", file.package());
    }
    for dep in &file.dependency {
        let _ = writeln!(out, "import \"{dep}\";");
    }
    for e in &file.enum_type {
        out.push('\n');
        render_enum(out, e, 0);
    }
    for m in &file.message_type {
        out.push('\n');
        render_message(out, m, 0);
    }
    for s in &file.service {
        out.push('\n');
        render_service(out, s);
    }
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn render_message(out: &mut String, msg: &DescriptorProto, level: usize) {
    indent(out, level);
    let _ = writeln!(out, "message {} {{", msg.name());

    for e in &msg.enum_type {
        render_enum(out, e, level + 1);
    }
    // Map fields use generated `*Entry` messages; show them as map<K, V>.
    for nested in &msg.nested_type {
        if !is_map_entry(nested) {
            render_message(out, nested, level + 1);
        }
    }

    // Fields in a real oneof are printed inside its block. Synthetic oneofs
    // (proto3 `optional`) are printed as plain `optional` fields.
    let mut printed_oneofs = vec![false; msg.oneof_decl.len()];
    for field in &msg.field {
        match real_oneof(field) {
            Some(idx) if idx < msg.oneof_decl.len() => {
                if printed_oneofs[idx] {
                    continue;
                }
                printed_oneofs[idx] = true;
                indent(out, level + 1);
                let _ = writeln!(out, "oneof {} {{", msg.oneof_decl[idx].name());
                for member in msg.field.iter().filter(|f| real_oneof(f) == Some(idx)) {
                    indent(out, level + 2);
                    let _ = writeln!(
                        out,
                        "{} {} = {};",
                        type_name(member, msg),
                        member.name(),
                        member.number()
                    );
                }
                indent(out, level + 1);
                out.push_str("}\n");
            }
            _ => {
                indent(out, level + 1);
                let _ = writeln!(
                    out,
                    "{}{} {} = {};",
                    label(field, msg),
                    type_name(field, msg),
                    field.name(),
                    field.number()
                );
            }
        }
    }

    let ranges: Vec<String> = msg
        .reserved_range
        .iter()
        // Descriptor ranges are end-exclusive; .proto syntax is inclusive.
        .map(|r| range_text(r.start(), r.end() - 1))
        .collect();
    render_reserved(out, &ranges, &msg.reserved_name, level + 1);

    indent(out, level);
    out.push_str("}\n");
}

fn render_enum(out: &mut String, e: &EnumDescriptorProto, level: usize) {
    indent(out, level);
    let _ = writeln!(out, "enum {} {{", e.name());
    for v in &e.value {
        indent(out, level + 1);
        let _ = writeln!(out, "{} = {};", v.name(), v.number());
    }
    // Enum reserved ranges are end-inclusive in the descriptor.
    let ranges: Vec<String> = e
        .reserved_range
        .iter()
        .map(|r| range_text(r.start(), r.end()))
        .collect();
    render_reserved(out, &ranges, &e.reserved_name, level + 1);
    indent(out, level);
    out.push_str("}\n");
}

fn render_service(out: &mut String, s: &ServiceDescriptorProto) {
    let _ = writeln!(out, "service {} {{", s.name());
    for m in &s.method {
        let stream = |yes: bool| if yes { "stream " } else { "" };
        let _ = writeln!(
            out,
            "  rpc {}({}{}) returns ({}{});",
            m.name(),
            stream(m.client_streaming()),
            short(m.input_type()),
            stream(m.server_streaming()),
            short(m.output_type()),
        );
    }
    out.push_str("}\n");
}

fn render_reserved(out: &mut String, ranges: &[String], names: &[String], level: usize) {
    if !ranges.is_empty() {
        indent(out, level);
        let _ = writeln!(out, "reserved {};", ranges.join(", "));
    }
    if !names.is_empty() {
        indent(out, level);
        let quoted: Vec<String> = names.iter().map(|n| format!("\"{n}\"")).collect();
        let _ = writeln!(out, "reserved {};", quoted.join(", "));
    }
}

fn range_text(start: i32, end_inclusive: i32) -> String {
    if start == end_inclusive {
        start.to_string()
    } else if end_inclusive >= 536_870_911 {
        format!("{start} to max")
    } else {
        format!("{start} to {end_inclusive}")
    }
}

fn real_oneof(field: &FieldDescriptorProto) -> Option<usize> {
    if field.proto3_optional() {
        return None;
    }
    field.oneof_index.map(|i| i as usize)
}

fn label(field: &FieldDescriptorProto, msg: &DescriptorProto) -> &'static str {
    if field.proto3_optional() {
        return "optional ";
    }
    match field.label() {
        Label::Repeated if map_entry_for(field, msg).is_some() => "",
        Label::Repeated => "repeated ",
        Label::Required => "required ",
        Label::Optional => "",
    }
}

fn type_name(field: &FieldDescriptorProto, msg: &DescriptorProto) -> String {
    if let Some(entry) = map_entry_for(field, msg) {
        let key = entry.field.iter().find(|f| f.number() == 1);
        let value = entry.field.iter().find(|f| f.number() == 2);
        if let (Some(k), Some(v)) = (key, value) {
            return format!("map<{}, {}>", type_name(k, entry), type_name(v, entry));
        }
    }
    match field.r#type() {
        Type::Message | Type::Enum | Type::Group => short(field.type_name()).to_string(),
        Type::Double => "double".into(),
        Type::Float => "float".into(),
        Type::Int64 => "int64".into(),
        Type::Uint64 => "uint64".into(),
        Type::Int32 => "int32".into(),
        Type::Fixed64 => "fixed64".into(),
        Type::Fixed32 => "fixed32".into(),
        Type::Bool => "bool".into(),
        Type::String => "string".into(),
        Type::Bytes => "bytes".into(),
        Type::Uint32 => "uint32".into(),
        Type::Sfixed32 => "sfixed32".into(),
        Type::Sfixed64 => "sfixed64".into(),
        Type::Sint32 => "sint32".into(),
        Type::Sint64 => "sint64".into(),
    }
}

/// Type references are fully qualified with a leading dot; drop the dot.
fn short(name: &str) -> &str {
    name.strip_prefix('.').unwrap_or(name)
}

fn is_map_entry(msg: &DescriptorProto) -> bool {
    msg.options.as_ref().is_some_and(|o| o.map_entry())
}

fn map_entry_for<'a>(
    field: &FieldDescriptorProto,
    msg: &'a DescriptorProto,
) -> Option<&'a DescriptorProto> {
    if field.label() != Label::Repeated || field.r#type() != Type::Message {
        return None;
    }
    let entry_name = field.type_name().rsplit('.').next()?;
    msg.nested_type
        .iter()
        .find(|n| n.name() == entry_name && is_map_entry(n))
}
