//! Read-only gRPC client for the game's telemetry server.
//!
//! Deliberately has no way to call `TriggerGameAction`: the recorder must
//! never be able to make the game do anything (spec §2). Only the replay
//! server knows that method.

use std::time::Duration;

use bytes::Bytes;
use http::uri::PathAndQuery;
use tonic::client::Grpc;
use tonic::transport::{Channel, Endpoint};
use tonic::{Request, Status, Streaming};

use crate::paths;
use crate::raw_codec::RawCodec;

pub struct TelemetryClient {
    grpc: Grpc<Channel>,
}

impl TelemetryClient {
    /// Opens an HTTP/2 connection. Fails fast so the caller's retry loop
    /// controls the pacing.
    pub async fn connect(endpoint: &str) -> anyhow::Result<Self> {
        let channel = Endpoint::from_shared(endpoint.to_string())?
            .connect_timeout(Duration::from_secs(2))
            .connect()
            .await?;
        Ok(Self {
            grpc: Grpc::new(channel),
        })
    }

    /// Returns the raw `ListTriggerableActionsResponse` bytes.
    pub async fn list_actions(&mut self) -> Result<Bytes, Status> {
        self.ready().await?;
        let response = self
            .grpc
            .unary(
                Request::new(Bytes::new()),
                PathAndQuery::from_static(paths::LIST_TRIGGERABLE_ACTIONS),
                RawCodec,
            )
            .await?;
        Ok(response.into_inner())
    }

    /// Opens the telemetry stream. Each item is one raw
    /// `TelemetryStreamResponse`.
    pub async fn telemetry_stream(&mut self) -> Result<Streaming<Bytes>, Status> {
        self.ready().await?;
        let response = self
            .grpc
            .server_streaming(
                Request::new(Bytes::new()),
                PathAndQuery::from_static(paths::TELEMETRY_STREAM),
                RawCodec,
            )
            .await?;
        Ok(response.into_inner())
    }

    async fn ready(&mut self) -> Result<(), Status> {
        self.grpc
            .ready()
            .await
            .map_err(|e| Status::unavailable(format!("channel not ready: {e}")))
    }
}
