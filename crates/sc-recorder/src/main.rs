//! sc-recorder entry point. With no arguments it records, so a double-click
//! on the gaming PC is all that's needed.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use sc_recorder::{DEFAULT_ENDPOINT, console, decode, record, replay, schema};

#[derive(Parser)]
#[command(
    name = "sc-recorder",
    version,
    about = "Records Star Citizen telemetry for the SC MQTT Companion"
)]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// `record` options, so `sc-recorder --minutes 5` works without the
    /// subcommand name.
    #[command(flatten)]
    record: RecordOpts,
}

#[derive(Subcommand)]
enum Command {
    /// Record telemetry from the running game (default).
    Record(RecordOpts),
    /// Decode a recording to JSONL and print a field summary.
    Decode {
        /// Recording zip or folder.
        input: PathBuf,
        /// Schema to use instead of the recording's schema.desc.
        #[arg(long)]
        desc: Option<PathBuf>,
        #[arg(long, default_value = "frames.jsonl")]
        out: PathBuf,
        /// Also write a copy with personal data removed (<name>-scrubbed.zip).
        #[arg(long)]
        scrub: bool,
    },
    /// Serve a recording as a fake game telemetry server.
    Replay {
        /// Recording zip or folder.
        input: PathBuf,
        #[arg(long, default_value = "127.0.0.1:2942")]
        listen: SocketAddr,
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
        /// Start over when the recording ends.
        #[arg(long = "loop")]
        looped: bool,
    },
    /// Extract the telemetry schema from StarCitizen.exe.
    ExtractSchema {
        exe: PathBuf,
        #[arg(long, default_value = "schema.desc")]
        out: PathBuf,
        /// Compare against another schema.desc and list changed fields.
        #[arg(long)]
        compare: Option<PathBuf>,
    },
}

#[derive(Args, Clone)]
struct RecordOpts {
    /// Stop after this many minutes (fractions allowed).
    #[arg(long)]
    minutes: Option<f64>,
    #[arg(long, default_value = DEFAULT_ENDPOINT)]
    endpoint: String,
    /// Where to create the recording folder (default: next to this exe).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Game exe to use instead of finding the running process.
    #[arg(long)]
    exe: Option<PathBuf>,
    /// Skip schema extraction.
    #[arg(long)]
    no_schema: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    // No arguments = started by double-click: keep the window open at the end.
    let interactive = std::env::args_os().len() == 1;
    let cli = Cli::parse();

    let result = match cli.command.unwrap_or(Command::Record(cli.record)) {
        Command::Record(opts) => {
            record::run(record::RecordArgs {
                minutes: opts.minutes,
                endpoint: opts.endpoint,
                out: opts.out,
                exe: opts.exe,
                no_schema: opts.no_schema,
                interactive,
            })
            .await
        }
        Command::Decode {
            input,
            desc,
            out,
            scrub,
        } => init_console().and_then(|_| {
            decode::run(decode::DecodeArgs {
                input,
                desc,
                out,
                scrub,
            })
        }),
        Command::Replay {
            input,
            listen,
            speed,
            looped,
        } => match init_console() {
            Ok(()) => {
                replay::run(replay::ReplayArgs {
                    input,
                    listen,
                    speed,
                    looped,
                })
                .await
            }
            Err(e) => Err(e),
        },
        Command::ExtractSchema { exe, out, compare } => {
            init_console().and_then(|_| schema::run_extract(&exe, &out, compare.as_deref()))
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            if interactive {
                console::wait_for_enter();
            }
            ExitCode::FAILURE
        }
    }
}

fn init_console() -> anyhow::Result<()> {
    console::init(None)?;
    Ok(())
}
