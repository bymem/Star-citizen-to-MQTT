//! Hand-written prost structs for the few messages we need typed access to.
//!
//! Field numbers for `ListTriggerableActionsResponse` come from the upstream
//! schema (spec §3). The `TriggerGameAction` messages are only used by the
//! replay server for logging; their layout is a best guess
//! (`action_name = 1`, `hold_ms = 2`, `status = 1`) and should be checked
//! against an extracted `schema.proto` once a real recording exists.

use serde::Serialize;

#[derive(Clone, PartialEq, prost::Message, Serialize)]
pub struct TriggerableAction {
    #[prost(string, tag = "1")]
    pub action_name: String,
    #[prost(string, tag = "2")]
    pub category: String,
    #[prost(bool, tag = "3")]
    pub holdable: bool,
}

#[derive(Clone, PartialEq, prost::Message, Serialize)]
pub struct ListTriggerableActionsResponse {
    #[prost(message, repeated, tag = "1")]
    pub actions: Vec<TriggerableAction>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TriggerGameActionRequest {
    #[prost(string, tag = "1")]
    pub action_name: String,
    #[prost(uint32, tag = "2")]
    pub hold_ms: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TriggerGameActionResponse {
    /// 1 = OK in the game's status enum.
    #[prost(int32, tag = "1")]
    pub status: i32,
}
