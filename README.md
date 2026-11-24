# Star Citizen to MQTT

A companion that publishes Star Citizen's local game telemetry to MQTT (for
Home Assistant and similar). Work in progress.

## Phase 0: sc-recorder

`crates/sc-recorder` captures real telemetry on the gaming PC so the rest of
the companion can be built on a machine without the game. See `spec.md` for
the full spec.

| Command | Where | What |
| --- | --- | --- |
| `sc-recorder` (or `record`) | Gaming PC | Waits for the game, records the telemetry stream, writes `sc-recording-<timestamp>.zip`. Read-only. |
| `sc-recorder extract-schema <StarCitizen.exe>` | Gaming PC | Pulls the telemetry `.proto` schema out of the exe. `--compare old.desc` lists changed fields. |
| `sc-recorder decode <rec.zip>` | Dev machine | Frames to JSONL plus a summary of every field seen. `--scrub` writes a copy without personal data. |
| `sc-recorder replay <rec.zip>` | Dev machine | Fake game server on `127.0.0.1:2942` that streams the recording. Logs `TriggerGameAction` calls. |

Player instructions ship in the Windows zip: `packaging/README.txt`.

## Development

Rust runs in Docker, so nothing is installed on the host:

```sh
./dev cargo test                       # unit + round-trip tests
./dev cargo clippy --all-targets -- -D warnings
./dev cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings   # type-check Windows code
./dev cargo run -p sc-recorder -- decode rec.zip
```

Build output and the cargo cache live in Docker volumes. Remove everything
with `docker compose down -v && docker rmi sc-mqtt-dev`.

To test against the real upstream schema instead of the built-in fake one,
set `SC_UPSTREAM_DESC=/path/to/sc_extrait.desc` when running the tests. Do not
commit that file.

## Releases

1. Bump `version` in `crates/sc-recorder/Cargo.toml` and commit.
2. Tag and push: `git tag v0.1.0 && git push origin v0.1.0`.
3. GitHub Actions builds Windows and Linux packages and publishes a Release
   with `SHA256SUMS.txt`. Tags with a suffix (`v0.2.0-rc.1`) become
   pre-releases.

Every push also runs CI and uploads a Windows test exe as a build artifact.

## Privacy

Recordings can contain your aUEC balance, ship names and your Windows user
name. Extracted schemas come from the game binary. Keep both out of public
repos; `.gitignore` already excludes `*.zip`, `sc-recording-*/` and `*.desc`.
