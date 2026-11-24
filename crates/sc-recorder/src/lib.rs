//! sc-recorder: captures Star Citizen's local gRPC telemetry on the gaming PC
//! and decodes/replays it on a dev machine without the game.
//!
//! The binary lives in `main.rs`; everything is a library so integration
//! tests can reuse the recording format and test helpers.

pub mod client;
pub mod console;
pub mod decode;
pub mod proto;
pub mod raw_codec;
pub mod record;
pub mod recording;
pub mod replay;
pub mod schema;
pub mod sections;

/// gRPC method paths. Requests for both client-side methods are empty
/// messages, so the recorder never needs the game's schema to talk to it.
pub mod paths {
    pub const SERVICE: &str = "sc.game.telemetry.v1.GameTelemetryService";
    pub const TELEMETRY_STREAM: &str = "/sc.game.telemetry.v1.GameTelemetryService/TelemetryStream";
    pub const LIST_TRIGGERABLE_ACTIONS: &str =
        "/sc.game.telemetry.v1.GameTelemetryService/ListTriggerableActions";
}

/// Full message name used to decode frames.
pub const TELEMETRY_RESPONSE: &str = "sc.game.telemetry.v1.TelemetryStreamResponse";

/// Default endpoint the game listens on.
pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:2942";
