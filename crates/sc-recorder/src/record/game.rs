//! Finds the running game and reads its version info. Read-only.

use std::fs;
use std::path::{Path, PathBuf};

use sysinfo::{ProcessesToUpdate, System};

use crate::recording::meta::GameInfo;

pub const PROCESS_NAME: &str = "StarCitizen.exe";

/// Polls the process list for the game. Kept as a struct so the `System`
/// cache is reused across polls instead of rebuilt every 2 s.
pub struct GameFinder {
    system: Option<System>,
}

impl Default for GameFinder {
    fn default() -> Self {
        Self::new()
    }
}

impl GameFinder {
    pub fn new() -> Self {
        Self {
            system: System::new().ok(),
        }
    }

    /// Path of the running `StarCitizen.exe`, if any.
    pub fn find(&mut self) -> Option<PathBuf> {
        let system = self.system.as_mut()?;
        system.refresh_processes(ProcessesToUpdate::All, true);
        system
            .processes_by_exact_name(PROCESS_NAME.as_ref())
            .find_map(|p| p.exe().map(Path::to_path_buf))
    }
}

/// Version info that does not require reading the whole exe.
/// The channel folder is the parent of `Bin64`, e.g. `...\StarCitizen\LIVE`.
pub fn read_game_info(exe: &Path) -> GameInfo {
    let channel_dir = exe
        .parent()
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("Bin64"))
        })
        .and_then(Path::parent);

    let mut info = GameInfo {
        exe_path: Some(exe.display().to_string()),
        channel: channel_dir
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().to_uppercase()),
        ..Default::default()
    };

    // Game.log is locked for writing by the game; std opens files with full
    // sharing on Windows, so a plain read-only open works alongside it.
    // A missing log is normal (fresh install, --exe pointing elsewhere).
    if let Some(log) = channel_dir.and_then(|d| fs::read(d.join("Game.log")).ok()) {
        let text = String::from_utf8_lossy(&log);
        let (file_version, build) = parse_game_log(&text);
        info.file_version = file_version;
        info.build = build;
    }
    info
}

/// Pulls `FileVersion:` and `Build(...)` from the log header.
pub fn parse_game_log(text: &str) -> (Option<String>, Option<String>) {
    let mut file_version = None;
    let mut build = None;
    for line in text.lines().take(500) {
        if file_version.is_none()
            && let Some((_, rest)) = line.split_once("FileVersion:")
        {
            file_version = Some(rest.trim().to_string());
        }
        if build.is_none()
            && let Some((_, rest)) = line.split_once("Build(")
        {
            build = rest.split_once(')').map(|(b, _)| b.trim().to_string());
        }
        if file_version.is_some() && build.is_some() {
            break;
        }
    }
    (file_version, build)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_log_header() {
        let log = "<2026-11-21T10:00:00.000Z> Log started\n\
                   <2026-11-21T10:00:00.001Z> FileVersion: 5.0.202.10025\n\
                   <2026-11-21T10:00:00.002Z> Branch: sc-alpha-5.0\n\
                   <2026-11-21T10:00:00.003Z> Build(13248297)\n";
        let (version, build) = parse_game_log(log);
        assert_eq!(version.as_deref(), Some("5.0.202.10025"));
        assert_eq!(build.as_deref(), Some("13248297"));
    }

    #[test]
    fn channel_from_path() {
        let exe = Path::new("/games/StarCitizen/PTU/Bin64/StarCitizen.exe");
        assert_eq!(read_game_info(exe).channel.as_deref(), Some("PTU"));
    }
}
