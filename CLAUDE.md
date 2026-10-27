# Backup Agent — instructions for Claude Code

SQL Server backup agent for Windows: a Windows service (`crates/service`), an egui
GUI (`crates/gui`) and shared core logic (`crates/core`, hexagonal: `domain`,
`ports`, `application`, `infrastructure`). The GUI talks to the service over a
named pipe (bincode IPC).

## Branches

- `main`: current Windows (10/11, Server 2016+), current stable Rust. New work
  starts here.
- `compat/windows-server-2012r2`: legacy build for Windows 7 SP1 / Server 2008 R2 /
  Server 2012 R2, pinned to Rust 1.77.2 and without S3. It is never merged into
  `main`; port fixes to it with `git cherry-pick -x` when the old machine needs them.

## Code constraints

- Config structs travel over bincode IPC: never use `#[serde(skip_serializing_if)]`
  on them (breaks deserialization). New config fields need `#[serde(default)]`
  so existing `config.toml` files keep loading.
- S3 storage is an optional feature: `cargo build --features backup-agent-core/s3`.

## Checks

```bat
cargo test --workspace
```

All tests must pass before building an installer.

## Versions and releases

- The version lives in two places that must match: `version` in the root
  `Cargo.toml` (`[workspace.package]`) and `AppVersion` in `installer/setup.iss`.
- Each release is a git tag `v<version>` with a GitHub Release carrying
  `installer\Output\BackupAgentSetup-<version>.exe`. Installers are never
  committed to git.

## Task: build the installer

Run from the repository root (cmd.exe):

1. `git switch main` and `git pull --ff-only` (ask first if the tree has local changes).
2. `cargo test --workspace` (stop and report on failure).
3. Build and stage the binaries:
   ```bat
   cargo build --release
   if not exist dist mkdir dist
   copy /Y target\release\backup-agent-service.exe dist\
   copy /Y target\release\backup-agent-gui.exe      dist\
   ```
4. `"C:\Program Files (x86)\Inno Setup 6\iscc.exe" installer\setup.iss`
   (if `iscc` is missing, ask the user to install Inno Setup 6 from
   https://jrsoftware.org/isdl.php).
5. Report the installer path, its timestamp and `git log -1 --oneline`.

## Upgrading an installation

- The installer replaces the two `.exe` files in `C:\Program Files\BackupAgent\`
  and stops/restarts the `BackupAgent` service. `config.toml` and `history.json`
  in that folder are not touched. Back up `config.toml` first.
- Saving from the GUI rewrites `config.toml` (comments are lost; the `[service]`
  section is preserved).
- Do not install, uninstall or restart services without the user's explicit OK.

## Google Drive mirror

- Settings → Storage → "Also copy to Google Drive", or `[storage.google_drive]` in
  `config.toml`; setup guide in `docs/google-drive-setup.md`.
- Local copy is written first; a Drive failure marks the job failed (Telegram
  alert) but keeps the local copy. `retention_days` also deletes that database's
  expired `<db>_YYYYMMDD_HHMMSS.zip` files from the Drive folder after a
  successful upload.
- To verify: trigger a manual backup and look for `Google Drive upload completed`
  in `backup-agent.log` next to the service exe. Never print OAuth secrets or tokens.
