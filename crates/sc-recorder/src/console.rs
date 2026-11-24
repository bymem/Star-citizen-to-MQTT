//! Console output: log lines, the single updating status line, and the
//! optional `recorder.log` copy.
//!
//! Log lines go through `tracing`. The console writer erases the status line
//! before printing so the two never end up on the same row.

use std::fmt;
use std::fs::File;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tracing::{Event, Level, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, MakeWriter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

/// True while an unfinished status line sits on the current console row.
static STATUS_SHOWN: AtomicBool = AtomicBool::new(false);
/// Whether `\r` + ANSI erase works on this console.
static VT_ENABLED: AtomicBool = AtomicBool::new(false);

/// Sets up logging to the console and, when given, to a log file.
/// Call once per process.
pub fn init(log_file: Option<&Path>) -> io::Result<()> {
    VT_ENABLED.store(enable_vt(), Ordering::Relaxed);

    let console = tracing_subscriber::fmt::layer()
        .event_format(PlainFormat)
        .with_writer(ConsoleWriter);
    let file = match log_file {
        Some(path) => Some(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(Mutex::new(File::create(path)?)),
        ),
        None => None,
    };
    let filter = tracing_subscriber::filter::LevelFilter::INFO;
    tracing_subscriber::registry()
        .with(filter)
        .with(console)
        .with(file)
        .init();
    Ok(())
}

/// The single updating status line. With VT support it is redrawn in place
/// every call; otherwise it prints as a normal line at most every 5 s.
pub struct StatusLine {
    last_plain: Option<Instant>,
}

impl Default for StatusLine {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusLine {
    pub fn new() -> Self {
        Self { last_plain: None }
    }

    pub fn show(&mut self, text: &str) {
        let mut out = io::stdout().lock();
        if VT_ENABLED.load(Ordering::Relaxed) {
            let _ = write!(out, "\r\x1b[2K{text}");
            STATUS_SHOWN.store(true, Ordering::Relaxed);
        } else {
            if self
                .last_plain
                .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
            {
                return;
            }
            self.last_plain = Some(Instant::now());
            let _ = writeln!(out, "{text}");
        }
        let _ = out.flush();
    }

    /// Ends the status row so following output starts on a fresh line.
    pub fn clear(&mut self) {
        clear_status();
        self.last_plain = None;
    }
}

fn clear_status() {
    if STATUS_SHOWN.swap(false, Ordering::Relaxed) {
        let mut out = io::stdout().lock();
        let _ = write!(out, "\r\x1b[2K");
        let _ = out.flush();
    }
}

/// Blocks until the user presses Enter (used after a double-click run so
/// the window does not vanish before the result can be read).
pub fn wait_for_enter() {
    clear_status();
    print!("\nPress Enter to close this window.");
    let _ = io::stdout().flush();
    let _ = io::stdin().read_line(&mut String::new());
}

/// Console writer that first erases a visible status line.
struct ConsoleWriter;

impl<'a> MakeWriter<'a> for ConsoleWriter {
    type Writer = ConsoleWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ConsoleWriter
    }
}

impl Write for ConsoleWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        clear_status();
        io::stdout().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stdout().flush()
    }
}

/// Console format meant for players, not developers: the message only,
/// with a prefix for warnings and errors.
struct PlainFormat;

impl<S, N> FormatEvent<S, N> for PlainFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        match *event.metadata().level() {
            Level::ERROR => write!(writer, "error: ")?,
            Level::WARN => write!(writer, "warning: ")?,
            _ => {}
        }
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

/// Turns on ANSI escape handling in the Windows console. Returns whether
/// in-place status updates can be used.
#[cfg(windows)]
fn enable_vt() -> bool {
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, STD_OUTPUT_HANDLE,
        SetConsoleMode,
    };
    if !io::stdout().is_terminal() {
        return false;
    }
    // SAFETY: plain Win32 calls on our own stdout handle; `mode` is a valid
    // out-pointer for the duration of the call.
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(handle, &mut mode) == 0 {
            return false;
        }
        SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

#[cfg(not(windows))]
fn enable_vt() -> bool {
    io::stdout().is_terminal()
}
