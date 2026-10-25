# Backup Agent — instructions for Claude Code

SQL Server backup agent for Windows: a Windows service (`crates/service`), an egui
GUI (`crates/gui`) and shared core logic (`crates/core`, hexagonal: `domain`,
`ports`, `application`, `infrastructure`). The GUI talks to the service over a
named pipe (bincode IPC).

## Branch and toolchain constraints (Windows Server 2012 R2)

- Work on `compat/windows-server-2012r2` or branches created from it
  (Google Drive mirror: `feat/google-drive-sync-2012r2`).
- Toolchain is pinned to **Rust 1.77.2** in `rust-toolchain.toml`: the last
  release that targets Windows 7 / 8 / 8.1 / Server 2012 R2. Never bump it.
- **Never run `cargo update`** and do not add dependencies that need Cargo 1.85+
  (edition 2024). `Cargo.lock` is load-bearing on this branch.
- No AWS SDK / S3 here (removed: incompatible with 1.77.2). Cloud uploads use
  plain `reqwest` (see `crates/core/src/infrastructure/storage/google_drive.rs`).
- Config structs travel over bincode IPC: never use `#[serde(skip_serializing_if)]`
  on them (breaks deserialization). New config fields need `#[serde(default)]`
  so existing `config.toml` files keep loading.

## Checks

```bat
cargo test --workspace
```

All tests must pass before building an installer.

## Task: build the installer on the server

Run from the repository root (cmd.exe). Ask the user before switching branches
if the working tree has local changes.

1. Update the code:
   ```bat
   git status
   git fetch origin
   git switch feat/google-drive-sync-2012r2
   git pull --ff-only
   ```
2. Verify the toolchain: `cargo --version` must print `cargo 1.77.2`.
   If Rust is missing, ask the user to install `rustup` (https://rustup.rs);
   it picks 1.77.2 automatically from `rust-toolchain.toml`.
3. Run the tests: `cargo test --workspace` (stop and report on failure).
4. Build and stage the binaries:
   ```bat
   cargo build --release
   if not exist dist mkdir dist
   copy /Y target\release\backup-agent-service.exe dist\
   copy /Y target\release\backup-agent-gui.exe      dist\
   ```
5. Build the installer (Inno Setup 6 must be installed; if `iscc` is missing,
   ask the user to install it from https://jrsoftware.org/isdl.php):
   ```bat
   "C:\Program Files (x86)\Inno Setup 6\iscc.exe" installer\setup.iss
   ```
   Output: `installer\Output\BackupAgentSetup.exe`.
6. Report the installer path, its timestamp and the commit it was built from
   (`git log -1 --oneline`).

## Task: upgrade an existing installation

- The installer only replaces the two `.exe` files in
  `C:\Program Files\BackupAgent\` and stops/restarts the `BackupAgent` service.
  `config.toml` and `history.json` live in that same folder and are NOT touched.
- Before running the installer, back up the config:
  ```bat
  copy "C:\Program Files\BackupAgent\config.toml" "%USERPROFILE%\config.toml.bak"
  ```
- Saving from the GUI rewrites `config.toml` entirely (comments are lost and the
  `[service]` section resets to defaults) — mention this to the user.
- Requires the Visual C++ 2015-2022 Redistributable (x64) on pre-Windows-10
  hosts (see `installer/README.md`).
- Do not install, uninstall or restart services without the user's explicit OK.

## Google Drive mirror

- Settings → "Also copy to Google Drive", or `[storage.google_drive]` in
  `config.toml` (see `config.example.toml` for the OAuth refresh-token setup).
- Local copy is written first; a Drive failure marks the job failed (Telegram
  alert) but keeps the local copy.
- Retention (`retention_days`) also cleans Drive: after a successful upload, that
  database's expired `<db>_YYYYMMDD_HHMMSS.zip` files in the target folder are
  permanently deleted (`GoogleDriveStorage::clean_old_backups`). Failed uploads
  never trigger Drive deletions.
- To verify after upgrading: trigger a manual backup and check the service log
  (`backup-agent.log` next to the service exe) for
  `Google Drive upload completed`. Never print OAuth secrets or tokens.
