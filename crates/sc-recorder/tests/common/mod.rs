//! Shared test helpers: a fake telemetry schema and synthetic frames.
//!
//! The real upstream schema can't be committed (unclear license), so tests
//! use this small stand-in with the same top-level layout. Set
//! `SC_UPSTREAM_DESC=/path/to/sc_extrait.desc` to generate frames from the
//! real schema instead.

#![allow(dead_code)]

use std::path::Path;

use bytes::Bytes;
use prost::Message;
use prost_reflect::{DynamicMessage, Kind, MessageDescriptor, Value};
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{
    DescriptorProto, EnumDescriptorProto, EnumValueDescriptorProto, FieldDescriptorProto,
    FileDescriptorProto, FileDescriptorSet, MethodDescriptorProto, OneofDescriptorProto,
    ServiceDescriptorProto, descriptor_proto::ReservedRange,
};
use sc_recorder::proto::{ListTriggerableActionsResponse, TriggerableAction};
use sc_recorder::recording::{FrameWriter, files};

pub const BIN: &str = env!("CARGO_BIN_EXE_sc-recorder");

fn field(name: &str, number: i32, ty: Type, type_name: Option<&str>) -> FieldDescriptorProto {
    FieldDescriptorProto {
        name: Some(name.into()),
        number: Some(number),
        label: Some(Label::Optional as i32),
        r#type: Some(ty as i32),
        type_name: type_name.map(Into::into),
        json_name: None,
        ..Default::default()
    }
}

fn repeated(mut f: FieldDescriptorProto) -> FieldDescriptorProto {
    f.label = Some(Label::Repeated as i32);
    f
}

fn message(name: &str, fields: Vec<FieldDescriptorProto>) -> DescriptorProto {
    DescriptorProto {
        name: Some(name.into()),
        field: fields,
        ..Default::default()
    }
}

/// Two files, so dependency ordering and imports are exercised, plus a
/// google/protobuf import to check well-known types resolve.
pub fn fake_schema() -> FileDescriptorSet {
    let common = FileDescriptorProto {
        name: Some("sc/game/telemetry/v1/common.proto".into()),
        package: Some("sc.game.telemetry.v1".into()),
        syntax: Some("proto3".into()),
        enum_type: vec![EnumDescriptorProto {
            name: Some("QuantumState".into()),
            value: [
                "QUANTUM_STATE_UNSPECIFIED",
                "QUANTUM_STATE_IDLE",
                "QUANTUM_STATE_SPOOLING",
                "QUANTUM_STATE_TRAVELLING",
            ]
            .iter()
            .enumerate()
            .map(|(i, n)| EnumValueDescriptorProto {
                name: Some((*n).into()),
                number: Some(i as i32),
                options: None,
            })
            .collect(),
            ..Default::default()
        }],
        message_type: vec![message(
            "Vec3",
            vec![
                field("x", 1, Type::Double, None),
                field("y", 2, Type::Double, None),
                field("z", 3, Type::Double, None),
            ],
        )],
        ..Default::default()
    };

    let mut actor = message(
        "ActorState",
        vec![
            field("uec_balance", 1, Type::Int64, None),
            field("health", 2, Type::Float, None),
            field("location", 3, Type::String, None),
            field("in_armistice", 4, Type::Bool, None),
        ],
    );
    // proto3 `optional` = a synthetic oneof.
    actor.field[3].proto3_optional = Some(true);
    actor.field[3].oneof_index = Some(0);
    actor.oneof_decl = vec![OneofDescriptorProto {
        name: Some("_in_armistice".into()),
        options: None,
    }];

    let mut event = message(
        "Event",
        vec![
            field("kind", 1, Type::String, None),
            field("text", 2, Type::String, None),
            field("amount", 3, Type::Int32, None),
        ],
    );
    event.field[1].oneof_index = Some(0);
    event.field[2].oneof_index = Some(0);
    event.oneof_decl = vec![OneofDescriptorProto {
        name: Some("detail".into()),
        options: None,
    }];

    let mut response = message(
        "TelemetryStreamResponse",
        vec![
            field(
                "vehicle",
                3,
                Type::Message,
                Some(".sc.game.telemetry.v1.VehicleState"),
            ),
            field(
                "weapons",
                4,
                Type::Message,
                Some(".sc.game.telemetry.v1.WeaponsState"),
            ),
            repeated(field(
                "events",
                5,
                Type::Message,
                Some(".sc.game.telemetry.v1.Event"),
            )),
            field(
                "actor",
                6,
                Type::Message,
                Some(".sc.game.telemetry.v1.ActorState"),
            ),
            field(
                "force_feedback",
                7,
                Type::Message,
                Some(".sc.game.telemetry.v1.ForceFeedback"),
            ),
            field(
                "turret",
                8,
                Type::Message,
                Some(".sc.game.telemetry.v1.TurretState"),
            ),
            field(
                "captured_at",
                9,
                Type::Message,
                Some(".google.protobuf.Timestamp"),
            ),
        ],
    );
    response.reserved_range = vec![ReservedRange {
        start: Some(1),
        end: Some(3),
    }];

    let telemetry = FileDescriptorProto {
        name: Some("sc/game/telemetry/v1/telemetry.proto".into()),
        package: Some("sc.game.telemetry.v1".into()),
        syntax: Some("proto3".into()),
        dependency: vec![
            "sc/game/telemetry/v1/common.proto".into(),
            "google/protobuf/timestamp.proto".into(),
        ],
        message_type: vec![
            message("TelemetryStreamRequest", vec![]),
            message(
                "VehicleState",
                vec![
                    field("name", 1, Type::String, None),
                    field("speed", 2, Type::Float, None),
                    field(
                        "quantum",
                        3,
                        Type::Enum,
                        Some(".sc.game.telemetry.v1.QuantumState"),
                    ),
                    field(
                        "position",
                        4,
                        Type::Message,
                        Some(".sc.game.telemetry.v1.Vec3"),
                    ),
                ],
            ),
            message("WeaponsState", vec![field("ammo", 1, Type::Int32, None)]),
            actor,
            event,
            message(
                "ForceFeedback",
                vec![field("intensity", 1, Type::Float, None)],
            ),
            message("TurretState", vec![field("yaw", 1, Type::Float, None)]),
            response,
        ],
        service: vec![ServiceDescriptorProto {
            name: Some("GameTelemetryService".into()),
            method: vec![MethodDescriptorProto {
                name: Some("TelemetryStream".into()),
                input_type: Some(".sc.game.telemetry.v1.TelemetryStreamRequest".into()),
                output_type: Some(".sc.game.telemetry.v1.TelemetryStreamResponse".into()),
                server_streaming: Some(true),
                ..Default::default()
            }],
            options: None,
        }],
        ..Default::default()
    };

    FileDescriptorSet {
        file: vec![common, telemetry],
    }
}

/// The schema to test with: upstream's if `SC_UPSTREAM_DESC` is set.
pub fn test_schema_bytes() -> Vec<u8> {
    match std::env::var_os("SC_UPSTREAM_DESC") {
        Some(path) => std::fs::read(path).expect("reading SC_UPSTREAM_DESC"),
        None => fake_schema().encode_to_vec(),
    }
}

/// Builds `n` distinct, partial frames by filling fields generically from
/// the descriptor, so it works for any schema version.
pub fn synthetic_frames(desc: &[u8], n: usize) -> Vec<Bytes> {
    let pool = sc_recorder::schema::load_pool(desc).unwrap();
    let response = sc_recorder::decode::find_response(&pool).unwrap();
    (0..n)
        .map(|i| Bytes::from(fill(&response, i, 0).encode_to_vec()))
        .collect()
}

fn fill(desc: &MessageDescriptor, i: usize, depth: usize) -> DynamicMessage {
    let mut msg = DynamicMessage::new(desc.clone());
    let mut oneofs_done = std::collections::HashSet::new();
    for field in desc.fields() {
        // Partial frames: skip some fields, varying per frame.
        if (i + field.number() as usize).is_multiple_of(3) && field.name() != "uec_balance" {
            continue;
        }
        if let Some(oneof) = field.containing_oneof()
            && !oneofs_done.insert(oneof.name().to_string())
        {
            continue;
        }
        if field.is_map() {
            continue;
        }
        let Some(value) = sample(&field.kind(), field.name(), i, depth) else {
            continue;
        };
        if field.is_list() {
            msg.set_field(&field, Value::List(vec![value.clone(), value]));
        } else {
            msg.set_field(&field, value);
        }
    }
    msg
}

fn sample(kind: &Kind, name: &str, i: usize, depth: usize) -> Option<Value> {
    let n = i as i64 + 1;
    Some(match kind {
        Kind::Double => Value::F64(n as f64 * 0.5),
        Kind::Float => Value::F32(n as f32 * 0.25),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => Value::I32(n as i32),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => Value::I64(n * 1000),
        Kind::Uint32 | Kind::Fixed32 => Value::U32(n as u32),
        Kind::Uint64 | Kind::Fixed64 => Value::U64(n as u64),
        Kind::Bool => Value::Bool(i.is_multiple_of(2)),
        Kind::String => Value::String(format!("{name}-{i}")),
        Kind::Bytes => Value::Bytes(Bytes::from(vec![i as u8; 3])),
        Kind::Enum(e) => {
            let values: Vec<_> = e.values().collect();
            Value::EnumNumber(values[i % values.len()].number())
        }
        Kind::Message(m) if depth < 4 => Value::Message(fill(m, i, depth + 1)),
        Kind::Message(_) => return None,
    })
}

/// Writes a recording folder (frames.bin, schema.desc, actions.bin) with
/// frames spaced `step_us` apart.
pub fn write_recording(dir: &Path, desc: &[u8], frames: &[Bytes], step_us: u64) {
    std::fs::create_dir_all(dir).unwrap();
    let mut w = FrameWriter::new(std::fs::File::create(dir.join(files::FRAMES)).unwrap()).unwrap();
    for (i, f) in frames.iter().enumerate() {
        w.write(i as u64 * step_us, 0, f).unwrap();
    }
    w.flush().unwrap();
    std::fs::write(dir.join(files::SCHEMA_DESC), desc).unwrap();
    let actions = ListTriggerableActionsResponse {
        actions: vec![
            TriggerableAction {
                action_name: "v_toggle_landing_system".into(),
                category: "vehicle".into(),
                holdable: false,
            },
            TriggerableAction {
                action_name: "v_toggle_quantum_mode".into(),
                category: "vehicle".into(),
                holdable: true,
            },
        ],
    };
    std::fs::write(dir.join(files::ACTIONS_BIN), actions.encode_to_vec()).unwrap();
}

/// A local port that is free right now.
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Kills the child process when dropped, so a failing test never leaves a
/// replay server running.
pub struct KillOnDrop(pub std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
