//! Schema extraction (spec §12.6): descriptors hidden among junk bytes at
//! random offsets must come back exactly.

mod common;

use prost::Message;
use prost_types::{FileDescriptorProto, FileDescriptorSet};
use sc_recorder::schema::{compare::compare, extract, render::render};

/// Deterministic pseudo-random bytes (no rand dependency needed).
fn junk(seed: &mut u64, len: usize) -> Vec<u8> {
    (0..len)
        .map(|_| {
            *seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (*seed >> 33) as u8
        })
        .collect()
}

fn fake_exe(set: &FileDescriptorSet) -> Vec<u8> {
    let mut seed = 42;
    let mut data = junk(&mut seed, 10_000);
    // A protobuf blob that must be ignored: not under sc/game/telemetry/.
    let otel = FileDescriptorProto {
        name: Some("opentelemetry/proto/common/v1/common.proto".into()),
        message_type: vec![prost_types::DescriptorProto {
            name: Some("AnyValue".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    data.extend(otel.encode_to_vec());
    for (i, file) in set.file.iter().enumerate() {
        data.extend(junk(&mut seed, 3_000 + i * 777));
        // Embedded twice, like a debug copy; one of them is followed by
        // bytes that look like a valid field 14 varint.
        data.extend(file.encode_to_vec());
        data.extend([0x70, 0x01]);
        data.extend(junk(&mut seed, 500));
        data.extend(file.encode_to_vec());
    }
    data.extend(junk(&mut seed, 5_000));
    data
}

#[test]
fn finds_embedded_descriptors() {
    let expected = common::fake_schema();
    let found = extract(&fake_exe(&expected));

    let names: Vec<_> = found.file.iter().map(|f| f.name()).collect();
    assert_eq!(
        names,
        [
            "sc/game/telemetry/v1/common.proto",
            "sc/game/telemetry/v1/telemetry.proto"
        ],
        "dependency first, opentelemetry skipped"
    );
    for file in &found.file {
        let original = expected
            .file
            .iter()
            .find(|f| f.name() == file.name())
            .unwrap();
        assert_eq!(file, original);
    }
    // Loads as a pool (imports, including google/protobuf, resolve).
    sc_recorder::schema::load_pool(&found.encode_to_vec()).unwrap();
}

#[test]
fn renders_readable_proto() {
    let text = render(&common::fake_schema());
    for needle in [
        "package sc.game.telemetry.v1;",
        "import \"google/protobuf/timestamp.proto\";",
        "optional bool in_armistice = 4;",
        "oneof detail {",
        "repeated sc.game.telemetry.v1.Event events = 5;",
        "reserved 1 to 2;",
        "rpc TelemetryStream(sc.game.telemetry.v1.TelemetryStreamRequest) returns (stream sc.game.telemetry.v1.TelemetryStreamResponse);",
        "QUANTUM_STATE_IDLE = 1;",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
    // The synthetic oneof for proto3 optional must not be printed.
    assert!(!text.contains("oneof _in_armistice"));
}

#[test]
fn compare_lists_changed_fields() {
    let a = common::fake_schema();
    let mut b = a.clone();
    let response = b.file[1]
        .message_type
        .iter_mut()
        .find(|m| m.name() == "VehicleState")
        .unwrap();
    response.field.retain(|f| f.number() != 2);
    response.field[0].name = Some("display_name".into());

    let report = compare(&a, &b, "a", "b");
    assert!(report.contains("Only in a: 1"), "{report}");
    assert!(
        report.contains("sc.game.telemetry.v1.VehicleState #2 -> speed"),
        "{report}"
    );
    assert!(report.contains("Only in b: 0"), "{report}");
    assert!(report.contains("#1: name -> display_name"), "{report}");
}

#[test]
fn reports_nothing_found() {
    let mut seed = 7;
    assert!(extract(&junk(&mut seed, 50_000)).file.is_empty());
}
