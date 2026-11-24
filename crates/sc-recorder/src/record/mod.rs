//! `record` subcommand: waits for the game, captures the telemetry stream
//! into a recording folder, then zips it.
//!
//! Strictly read-only towards the game: it reads the exe and `Game.log`,
//! and talks to `127.0.0.1:2942` through a client that can only list
//! actions and stream telemetry.

pub mod game;

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use bytes::Bytes;
use chrono::{Local, SecondsFormat};
use prost::Message;
use tokio::sync::watch;
use tokio::time::{MissedTickBehavior, interval, sleep};
use tonic::Streaming;

use crate::client::TelemetryClient;
use crate::console::{self, StatusLine};
use crate::proto::ListTriggerableActionsResponse;
use crate::recording::meta::{Meta, SchemaInfo, Segment};
use crate::recording::{FrameWriter, archive, files};
use crate::schema;
use crate::sections::{self, SECTIONS};

const GAME_POLL: Duration = Duration::from_secs(2);
const CONNECT_RETRY: Duration = Duration::from_secs(3);
const USER_CFG_HINT_AFTER: Duration = Duration::from_secs(60);
const SPAWN_HINT_AFTER: Duration = Duration::from_secs(10);

pub struct RecordArgs {
    pub minutes: Option<f64>,
    pub endpoint: String,
    pub out: Option<PathBuf>,
    pub exe: Option<PathBuf>,
    pub no_schema: bool,
    /// Started by double-click: wait for Enter before exiting.
    pub interactive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopReason {
    CtrlC,
    #[cfg_attr(not(windows), allow(dead_code))] // Ctrl+Break only exists on Windows.
    CtrlBreak,
    ConsoleClosed,
    TimeLimit,
}

type StopRx = watch::Receiver<Option<StopReason>>;

pub async fn run(args: RecordArgs) -> anyhow::Result<()> {
    let start = Instant::now();
    let started_at = Local::now();

    let base = match &args.out {
        Some(dir) => dir.clone(),
        // Next to the exe, so a double-clicked download keeps everything
        // in one place and nothing lands in AppData or the game folder.
        None => std::env::current_exe()?
            .parent()
            .context("exe has no parent folder")?
            .to_path_buf(),
    };
    let folder = base.join(format!(
        "sc-recording-{}",
        started_at.format("%Y%m%d-%H%M%S")
    ));
    fs::create_dir_all(&folder).with_context(|| format!("creating {}", folder.display()))?;
    console::init(Some(&folder.join(files::LOG)))?;
    print_banner(&folder);

    let (stop_tx, mut stop) = watch::channel(None);
    let minutes = args.minutes;
    tokio::spawn(async move {
        let reason = wait_for_stop(minutes).await;
        let _ = stop_tx.send(Some(reason));
        // Keep the sender alive so receivers never see a closed channel.
        std::future::pending::<()>().await;
    });

    let mut recorder = Recorder::new(
        folder,
        start,
        started_at.to_rfc3339_opts(SecondsFormat::Secs, false),
    )?;
    let mut status = StatusLine::new();
    recorder.run(&args, &mut status, &mut stop).await;

    let reason = (*stop.borrow()).unwrap_or(StopReason::CtrlC);
    status.clear();
    tracing::info!("Stopping ({}).", reason_text(reason));
    recorder.finish()?;

    // Windows kills the process ~5 s after the window is closed, so never
    // wait for input in that case.
    if args.interactive && reason != StopReason::ConsoleClosed {
        console::wait_for_enter();
    }
    Ok(())
}

struct Recorder {
    folder: PathBuf,
    start: Instant,
    writer: FrameWriter<File>,
    meta: Meta,
    /// Index of the next segment to open.
    next_segment: u16,
    /// Frames per section, same order as `SECTIONS`.
    section_frames: [u64; SECTIONS.len()],
}

impl Recorder {
    fn new(folder: PathBuf, start: Instant, started_at: String) -> anyhow::Result<Self> {
        let writer = FrameWriter::new(File::create(folder.join(files::FRAMES))?)?;
        let meta = Meta {
            format: 1,
            recorder_version: env!("CARGO_PKG_VERSION").to_string(),
            started_at,
            ..Default::default()
        };
        let recorder = Self {
            folder,
            start,
            writer,
            meta,
            next_segment: 0,
            section_frames: [0; SECTIONS.len()],
        };
        recorder.save_meta();
        Ok(recorder)
    }

    fn now_us(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }

    /// Main loop: find game, connect, capture, repeat until stopped.
    async fn run(&mut self, args: &RecordArgs, status: &mut StatusLine, stop: &mut StopRx) {
        let mut finder = game::GameFinder::new();
        let mut identified = false;
        loop {
            let exe = match &args.exe {
                Some(path) => path.clone(),
                None => tokio::select! {
                    exe = wait_for_game(&mut finder, status) => exe,
                    _ = stopped(stop) => return,
                },
            };

            // Version and schema come from the first game seen; a restart
            // mid-recording is almost always the same build.
            if !identified {
                identified = true;
                tokio::select! {
                    _ = self.identify_game(&exe, args.no_schema) => {}
                    _ = stopped(stop) => return,
                }
            }

            let client = tokio::select! {
                c = connect(&args.endpoint, &exe, status) => c,
                _ = stopped(stop) => return,
            };
            self.capture(client, status, stop).await;
            if stop.borrow().is_some() {
                return;
            }
            // Short pause so a closing game is gone before we look again.
            tokio::select! {
                _ = sleep(GAME_POLL) => {}
                _ = stopped(stop) => return,
            }
        }
    }

    /// Reads version info, hashes the exe and extracts the schema.
    async fn identify_game(&mut self, exe: &Path, no_schema: bool) {
        tracing::info!("Game: {}", exe.display());
        let exe = exe.to_path_buf();
        let folder = self.folder.clone();
        let result =
            tokio::task::spawn_blocking(move || identify_blocking(&exe, &folder, no_schema)).await;
        match result {
            Ok((game, schema)) => {
                if let Some(v) = &game.file_version {
                    tracing::info!(
                        "Version: {v} (build {})",
                        game.build.as_deref().unwrap_or("?")
                    );
                }
                self.meta.game = game;
                self.meta.schema = schema;
            }
            Err(e) => tracing::warn!("Could not read game info: {e}"),
        }
        self.save_meta();
    }

    /// One connection = one segment. Returns when the stream ends or the
    /// recording is stopped.
    async fn capture(
        &mut self,
        mut client: TelemetryClient,
        status: &mut StatusLine,
        stop: &mut StopRx,
    ) {
        let index = self.next_segment;
        let segment_start = Instant::now();
        self.meta.segments.push(Segment {
            index,
            connected_at_us: self.now_us(),
            ..Default::default()
        });
        status.clear();
        tracing::info!("Connected to telemetry server (segment {index}).");

        match client.list_actions().await {
            Ok(raw) => self.save_actions(&raw),
            Err(e) => tracing::warn!("ListTriggerableActions failed: {}", e.message()),
        }

        let open = client.telemetry_stream();
        tokio::pin!(open);
        let mut stream: Option<Streaming<Bytes>> = None;
        let mut tick = interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut spawn_hint_shown = false;
        let mut fps = FpsCounter::new(self.meta.totals.frames);

        let end_reason = loop {
            tokio::select! {
                _ = stopped(stop) => break "recording stopped".to_string(),
                opened = &mut open, if stream.is_none() => match opened {
                    Ok(s) => stream = Some(s),
                    Err(e) => break format!("could not open stream: {:?}: {}", e.code(), e.message()),
                },
                msg = next_message(&mut stream), if stream.is_some() => match msg {
                    Ok(Some(frame)) => self.on_frame(index, &frame),
                    Ok(None) => break "stream ended by server".to_string(),
                    Err(e) => break format!("stream error: {:?}: {}", e.code(), e.message()),
                },
                _ = tick.tick() => {
                    self.flush();
                    let seg_frames = self.meta.segments.last().map_or(0, |s| s.frames);
                    if seg_frames == 0 && !spawn_hint_shown && segment_start.elapsed() >= SPAWN_HINT_AFTER {
                        spawn_hint_shown = true;
                        status.clear();
                        tracing::info!("Connected. Waiting for your character to spawn…");
                    }
                    status.show(&self.status_text(fps.update(self.meta.totals.frames)));
                }
            }
        };

        self.flush();
        status.clear();
        let now = self.now_us();
        let segment = self.meta.segments.last_mut().expect("segment pushed above");
        segment.ended_at_us = Some(now);
        segment.end_reason = Some(end_reason.clone());
        let frames = segment.frames;
        if stream.is_none() && frames == 0 {
            // Never got a stream: not a real session. Drop it so failed
            // attempts don't pile up as empty segments; the index is reused.
            self.meta.segments.pop();
            tracing::warn!("Telemetry server did not stream ({end_reason}).");
        } else {
            self.next_segment += 1;
            tracing::info!("Segment {index} ended after {frames} frames: {end_reason}.");
        }
        self.save_meta();
    }

    fn on_frame(&mut self, segment: u16, frame: &[u8]) {
        let t_us = self.now_us();
        if let Err(e) = self.writer.write(t_us, segment, frame) {
            tracing::error!("Writing frames.bin failed: {e}");
        }
        let set = sections::scan(frame);
        for (i, (field, _)) in SECTIONS.iter().enumerate() {
            if set.contains(*field) {
                self.section_frames[i] += 1;
            }
        }
        self.meta.totals.frames += 1;
        self.meta.totals.bytes += frame.len() as u64;
        if let Some(seg) = self.meta.segments.last_mut() {
            seg.frames += 1;
            seg.first_frame_at_us.get_or_insert(t_us);
        }
    }

    fn save_actions(&mut self, raw: &[u8]) {
        let write = || -> anyhow::Result<usize> {
            fs::write(self.folder.join(files::ACTIONS_BIN), raw)?;
            let decoded = ListTriggerableActionsResponse::decode(raw)?;
            fs::write(
                self.folder.join(files::ACTIONS_JSON),
                serde_json::to_vec_pretty(&decoded)?,
            )?;
            Ok(decoded.actions.len())
        };
        match write() {
            Ok(count) => {
                tracing::info!("Saved {count} triggerable actions.");
                self.meta.actions_count = Some(count);
            }
            Err(e) => tracing::warn!("Could not save actions: {e}"),
        }
    }

    fn flush(&mut self) {
        if let Err(e) = self.writer.flush() {
            tracing::error!("Flushing frames.bin failed: {e}");
        }
    }

    fn save_meta(&self) {
        let mut meta = self.meta.clone();
        meta.totals.sections_seen = SECTIONS
            .iter()
            .zip(self.section_frames)
            .map(|((_, name), n)| (name.to_string(), n))
            .collect();
        if let Err(e) = meta.write_atomic(&self.folder.join(files::META)) {
            tracing::error!("Writing meta.json failed: {e}");
        }
    }

    fn status_text(&self, fps: f64) -> String {
        let secs = self.start.elapsed().as_secs();
        let mark = |name: &str| {
            let i = SECTIONS.iter().position(|(_, n)| *n == name).unwrap();
            if self.section_frames[i] > 0 {
                "✓"
            } else {
                "–"
            }
        };
        let events =
            self.section_frames[SECTIONS.iter().position(|(_, n)| *n == "events").unwrap()];
        format!(
            "Recording {:02}:{:02} | {} frames | {fps:.1} fps | actor {} vehicle {} weapons {} events {events} ff {} turret {}",
            secs / 60,
            secs % 60,
            group_thousands(self.meta.totals.frames),
            mark("actor"),
            mark("vehicle"),
            mark("weapons"),
            mark("force_feedback"),
            mark("turret"),
        )
    }

    /// Flushes, writes final metadata, zips and prints the summary.
    fn finish(mut self) -> anyhow::Result<()> {
        self.flush();
        self.meta.ended_at = Some(Local::now().to_rfc3339_opts(SecondsFormat::Secs, false));
        self.save_meta();
        // Close frames.bin before zipping it.
        drop(self.writer.into_inner()?);

        tracing::info!(
            "Recorded {} frames in {} segment(s).",
            group_thousands(self.meta.totals.frames),
            self.meta.segments.len()
        );
        let covered: Vec<String> = SECTIONS
            .iter()
            .zip(self.section_frames)
            .map(|((_, name), n)| format!("{name} {}", if n > 0 { "✓" } else { "✗" }))
            .collect();
        tracing::info!("Covered: {}", covered.join(" "));
        for ((_, name), n) in SECTIONS.iter().zip(self.section_frames) {
            if n == 0 {
                tracing::info!("  Tip: no {name} data. {}", coverage_tip(name));
            }
        }

        tracing::info!("Zipping…");
        match archive::zip_dir(&self.folder) {
            Ok(zip) => {
                let size = fs::metadata(&zip).map(|m| m.len()).unwrap_or(0);
                tracing::info!(
                    "Done. Send this file: {} ({:.1} MB)",
                    zip.display(),
                    size as f64 / 1_000_000.0
                );
            }
            Err(e) => tracing::error!(
                "Zipping failed ({e}). The folder {} is complete; send it instead.",
                self.folder.display()
            ),
        }
        Ok(())
    }
}

fn identify_blocking(
    exe: &Path,
    folder: &Path,
    no_schema: bool,
) -> (crate::recording::meta::GameInfo, SchemaInfo) {
    let mut game = game::read_game_info(exe);
    let mut schema_info = SchemaInfo::default();
    let data = match fs::read(exe) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("Could not read {}: {e}", exe.display());
            return (game, schema_info);
        }
    };
    game.exe_sha256 = Some(schema::sha256_hex(&data));
    if no_schema {
        return (game, schema_info);
    }

    tracing::info!("Extracting telemetry schema from the game…");
    let set = schema::extract(&data);
    if set.file.is_empty() {
        tracing::warn!("{}", schema::extract::NOT_FOUND_HINT);
        return (game, schema_info);
    }
    match schema::write_schema(&set, &folder.join(files::SCHEMA_DESC)) {
        Ok((desc_path, _)) => {
            schema_info.extracted = true;
            schema_info.files = set.file.iter().map(|f| f.name().to_string()).collect();
            schema_info.sha256 = fs::read(desc_path).ok().map(|d| schema::sha256_hex(&d));
            tracing::info!("Schema extracted ({} files).", schema_info.files.len());
        }
        Err(e) => tracing::warn!("Could not save schema: {e}"),
    }
    (game, schema_info)
}

async fn wait_for_game(finder: &mut game::GameFinder, status: &mut StatusLine) -> PathBuf {
    loop {
        if let Some(exe) = finder.find() {
            status.clear();
            return exe;
        }
        status.show("Waiting for Star Citizen to start…");
        sleep(GAME_POLL).await;
    }
}

/// Retries until the telemetry server accepts an HTTP/2 connection.
async fn connect(endpoint: &str, exe: &Path, status: &mut StatusLine) -> TelemetryClient {
    let begun = Instant::now();
    let mut hint_shown = false;
    loop {
        match TelemetryClient::connect(endpoint).await {
            Ok(client) => {
                status.clear();
                return client;
            }
            Err(e) => tracing::debug!("connect failed: {e}"),
        }
        if !hint_shown && begun.elapsed() >= USER_CFG_HINT_AFTER {
            hint_shown = true;
            status.clear();
            print_user_cfg_hint(exe);
        }
        status.show("Waiting for telemetry server… (is telemetry enabled in user.cfg?)");
        sleep(CONNECT_RETRY).await;
    }
}

async fn next_message(
    stream: &mut Option<Streaming<Bytes>>,
) -> Result<Option<Bytes>, tonic::Status> {
    match stream {
        Some(s) => s.message().await,
        None => std::future::pending().await,
    }
}

/// Resolves once a stop has been requested.
async fn stopped(stop: &mut StopRx) {
    let _ = stop.wait_for(Option::is_some).await;
}

async fn wait_for_stop(minutes: Option<f64>) -> StopReason {
    let time_limit = async {
        match minutes {
            Some(m) => sleep(Duration::from_secs_f64(m.max(0.0) * 60.0)).await,
            None => std::future::pending().await,
        }
    };

    #[cfg(windows)]
    {
        use tokio::signal::windows;
        let (Ok(mut ctrl_break), Ok(mut ctrl_close)) =
            (windows::ctrl_break(), windows::ctrl_close())
        else {
            tracing::warn!("Could not install console handlers; use Ctrl+C to stop.");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => return StopReason::CtrlC,
                _ = time_limit => return StopReason::TimeLimit,
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => StopReason::CtrlC,
            _ = ctrl_break.recv() => StopReason::CtrlBreak,
            _ = ctrl_close.recv() => StopReason::ConsoleClosed,
            _ = time_limit => StopReason::TimeLimit,
        }
    }

    #[cfg(not(windows))]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).ok();
        let sigterm = async {
            match term.as_mut() {
                Some(s) => {
                    s.recv().await;
                }
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => StopReason::CtrlC,
            // SIGTERM behaves like closing the console window.
            _ = sigterm => StopReason::ConsoleClosed,
            _ = time_limit => StopReason::TimeLimit,
        }
    }
}

fn reason_text(reason: StopReason) -> &'static str {
    match reason {
        StopReason::CtrlC => "Ctrl+C",
        StopReason::CtrlBreak => "Ctrl+Break",
        StopReason::ConsoleClosed => "window closed",
        StopReason::TimeLimit => "time limit reached",
    }
}

fn print_banner(folder: &Path) {
    tracing::info!("sc-recorder {}", env!("CARGO_PKG_VERSION"));
    tracing::info!("Records Star Citizen telemetry for the SC MQTT Companion.");
    tracing::info!("Read-only: it never changes the game, its files or your settings.");
    tracing::info!("Saving to {}", folder.display());
    tracing::info!("Press Ctrl+C or close this window to stop.");
    tracing::info!("");
}

fn print_user_cfg_hint(exe: &Path) {
    let cfg = exe
        .parent()
        .and_then(Path::parent)
        .map(|d| d.join("user.cfg").display().to_string())
        .unwrap_or_else(|| {
            r"C:\Program Files\Roberts Space Industries\StarCitizen\LIVE\user.cfg".into()
        });
    tracing::warn!("Still no telemetry server after 60 seconds.");
    tracing::warn!("Telemetry must be enabled in the game. Add this line to {cfg}:");
    tracing::warn!("    server.game.telemetry.enabled=1");
    tracing::warn!(
        "Then restart Star Citizen. This tool will keep waiting; it does not edit the file."
    );
}

fn coverage_tip(section: &str) -> &'static str {
    match section {
        "vehicle" => "Sit in a ship's pilot seat and fly for a bit.",
        "weapons" => "Draw a weapon or use ship weapons.",
        "events" => "Do things that trigger events: quantum travel, landing, taking damage.",
        "actor" => "Make sure your character has spawned.",
        "force_feedback" => "Fly a ship and take some hits or boost.",
        "turret" => "Sit in a turret seat.",
        _ => "",
    }
}

/// `4301` -> `4 301`.
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Frames per second over the last status tick.
struct FpsCounter {
    last_frames: u64,
    last_at: Instant,
}

impl FpsCounter {
    fn new(frames: u64) -> Self {
        Self {
            last_frames: frames,
            last_at: Instant::now(),
        }
    }

    fn update(&mut self, frames: u64) -> f64 {
        let secs = self.last_at.elapsed().as_secs_f64();
        let fps = if secs > 0.0 {
            (frames - self.last_frames) as f64 / secs
        } else {
            0.0
        };
        self.last_frames = frames;
        self.last_at = Instant::now();
        fps
    }
}

#[cfg(test)]
mod tests {
    use super::group_thousands;

    #[test]
    fn groups_thousands() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(4301), "4 301");
        assert_eq!(group_thousands(1_234_567), "1 234 567");
    }
}
