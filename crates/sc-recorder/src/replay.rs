//! `replay` subcommand: serves a recording as a fake game telemetry server.
//!
//! Uses the same method paths and a raw-bytes codec, so clients see exactly
//! the bytes the game sent. This is also the only place that knows
//! `TriggerGameAction`, so companion buttons can be tested without the game.

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use prost::Message;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};
use tokio_stream::wrappers::ReceiverStream;
use tonic::body::Body;
use tonic::server::{NamedService, ServerStreamingService, UnaryService};
use tonic::{Request, Response, Status};

use crate::paths;
use crate::proto::{TriggerGameActionRequest, TriggerGameActionResponse};
use crate::raw_codec::RawCodec;
use crate::recording::{FrameRecord, Recording};

const TRIGGER_GAME_ACTION: &str = "/sc.game.telemetry.v1.GameTelemetryService/TriggerGameAction";

/// Gaps between segments (game restarts) can be minutes long; replay them
/// as a short pause instead.
const MAX_SEGMENT_GAP_US: u64 = 1_000_000;

pub struct ReplayArgs {
    pub input: PathBuf,
    pub listen: SocketAddr,
    pub speed: f64,
    pub looped: bool,
}

struct ReplayData {
    frames: Vec<FrameRecord>,
    actions: Bytes,
    speed: f64,
    looped: bool,
    next_client: AtomicU64,
}

pub async fn run(args: ReplayArgs) -> anyhow::Result<()> {
    anyhow::ensure!(args.speed > 0.0, "--speed must be greater than 0");
    let rec = Recording::open(&args.input)?;
    let (frames, truncated) = rec.all_frames()?;
    anyhow::ensure!(!frames.is_empty(), "recording has no frames");
    if truncated > 0 {
        tracing::warn!("Ignored {truncated} bytes of an incomplete last record.");
    }
    if rec.actions_bin.is_none() {
        tracing::warn!(
            "Recording has no actions.bin; ListTriggerableActions will return an empty list."
        );
    }

    let duration = frames.last().map_or(0, |f| f.t_us) as f64 / 1e6;
    tracing::info!(
        "Replaying {} frames ({duration:.0} s recorded) on {} at {}x{}. Ctrl+C to stop.",
        frames.len(),
        args.listen,
        args.speed,
        if args.looped { ", looping" } else { "" }
    );

    let service = ReplayService {
        data: Arc::new(ReplayData {
            frames,
            actions: rec.actions_bin.unwrap_or_default(),
            speed: args.speed,
            looped: args.looped,
            next_client: AtomicU64::new(1),
        }),
    };
    tonic::transport::Server::builder()
        .add_service(service)
        .serve_with_shutdown(args.listen, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Hand-written equivalent of tonic's generated server: routes by path.
#[derive(Clone)]
struct ReplayService {
    data: Arc<ReplayData>,
}

impl NamedService for ReplayService {
    const NAME: &'static str = paths::SERVICE;
}

impl tower::Service<http::Request<Body>> for ReplayService {
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = BoxFuture<Result<Self::Response, Infallible>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<Body>) -> Self::Future {
        let data = self.data.clone();
        Box::pin(async move {
            let mut grpc = tonic::server::Grpc::new(RawCodec);
            let response = match req.uri().path() {
                paths::TELEMETRY_STREAM => grpc.server_streaming(StreamMethod(data), req).await,
                paths::LIST_TRIGGERABLE_ACTIONS => grpc.unary(ListMethod(data), req).await,
                TRIGGER_GAME_ACTION => grpc.unary(TriggerMethod, req).await,
                other => Status::unimplemented(format!("unknown method {other}")).into_http(),
            };
            Ok(response)
        })
    }
}

struct StreamMethod(Arc<ReplayData>);

impl ServerStreamingService<Bytes> for StreamMethod {
    type Response = Bytes;
    type ResponseStream = ReceiverStream<Result<Bytes, Status>>;
    type Future = BoxFuture<Result<Response<Self::ResponseStream>, Status>>;

    fn call(&mut self, _request: Request<Bytes>) -> Self::Future {
        let data = self.0.clone();
        Box::pin(async move {
            // Each client gets its own task, so its own replay cursor.
            let (tx, rx) = mpsc::channel(32);
            tokio::spawn(stream_frames(data, tx));
            Ok(Response::new(ReceiverStream::new(rx)))
        })
    }
}

async fn stream_frames(data: Arc<ReplayData>, tx: mpsc::Sender<Result<Bytes, Status>>) {
    let id = data.next_client.fetch_add(1, Ordering::Relaxed);
    tracing::info!("Client {id}: TelemetryStream started.");
    loop {
        // Schedule against a fixed base so sleep jitter doesn't add up.
        let base = Instant::now();
        let mut offset_us = 0f64;
        let mut prev: Option<&FrameRecord> = None;
        for frame in &data.frames {
            if let Some(p) = prev {
                let mut gap = frame.t_us.saturating_sub(p.t_us);
                if frame.segment != p.segment {
                    gap = gap.min(MAX_SEGMENT_GAP_US);
                }
                offset_us += gap as f64 / data.speed;
            }
            sleep_until(base + Duration::from_micros(offset_us as u64)).await;
            if tx.send(Ok(frame.payload.clone())).await.is_err() {
                tracing::info!("Client {id}: disconnected.");
                return;
            }
            prev = Some(frame);
        }
        if !data.looped {
            break;
        }
    }
    // Dropping `tx` ends the stream with status OK, like a game shutdown.
    tracing::info!("Client {id}: end of recording, closing stream.");
}

struct ListMethod(Arc<ReplayData>);

impl UnaryService<Bytes> for ListMethod {
    type Response = Bytes;
    type Future = BoxFuture<Result<Response<Bytes>, Status>>;

    fn call(&mut self, _request: Request<Bytes>) -> Self::Future {
        let actions = self.0.actions.clone();
        Box::pin(async move { Ok(Response::new(actions)) })
    }
}

struct TriggerMethod;

impl UnaryService<Bytes> for TriggerMethod {
    type Response = Bytes;
    type Future = BoxFuture<Result<Response<Bytes>, Status>>;

    fn call(&mut self, request: Request<Bytes>) -> Self::Future {
        Box::pin(async move {
            match TriggerGameActionRequest::decode(request.into_inner()) {
                Ok(req) => tracing::info!(
                    "TriggerGameAction: {} (hold_ms {})",
                    req.action_name,
                    req.hold_ms
                ),
                Err(e) => tracing::warn!("TriggerGameAction with undecodable request: {e}"),
            }
            let reply = TriggerGameActionResponse { status: 1 };
            Ok(Response::new(Bytes::from(reply.encode_to_vec())))
        })
    }
}
