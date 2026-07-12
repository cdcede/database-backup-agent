# Backup Agent — Windows Installer

This directory contains the Inno Setup 6 script that builds the Windows
`.exe` installer for Backup Agent.

## This branch: Windows Server 2012 R2 / Windows 7+ compatibility

This branch (`compat/windows-server-2012r2`) targets older Windows versions
(Windows 7 / Server 2008 R2 through Windows 8.1 / Server 2012 R2), which the
`main` branch does **not** support. Two things differ here from `main`:

1. **Pinned Rust toolchain.** Rust 1.78+ raised the minimum supported Windows
   version for `*-pc-windows-msvc` to Windows 10, so binaries built with a
   modern Rust toolchain will not even launch on Server 2012 R2. This branch
   pins the workspace to **Rust 1.77.2** via `rust-toolchain.toml` — the last
   release that still targets Windows 7/8/8.1 (`rustup` picks it up
   automatically; no manual `+1.77.2` flag needed). Because of this, several
   dependencies are pinned to older, edition-2021-compatible versions in
   `Cargo.lock` (newer releases of `clap`, `indexmap`, `image`, `quinn`, etc.
   require Cargo 1.85+ to even parse their manifest). Treat this lockfile as
   load-bearing: avoid running a plain `cargo update` on this branch, since it
   will happily re-pick incompatible versions cargo 1.77.2 can't build.
2. **No S3 storage support.** `aws-sdk-s3` and its transitive dependencies
   (`aws-sigv4`, etc.) are a major source of edition-2024-only releases, so
   the `s3` Cargo feature was removed entirely on this branch. Local storage
   is unaffected.
3. **Visual C++ Redistributable required on the target machine.** Rust/MSVC
   binaries link against the Universal C Runtime (`api-ms-win-crt-*.dll`),
   which ships built into Windows 10+ but is **not** present out of the box
   on Windows 7/8/8.1/Server 2012 R2. Install the
   [Visual C++ 2015-2022 Redistributable (x64)](https://aka.ms/vs/17/release/vc_redist.x64.exe)
   on the target machine before running the installer — `setup.iss` checks
   for it and warns (non-blocking) if it looks missing.

## Prerequisites

- **Windows 7 / Server 2008 R2 or newer** (target platform on this branch;
  `main` requires Windows 10+)
- **Rust 1.77.2** (installed automatically via `rustup` from
  `rust-toolchain.toml` when you run `cargo build` in the repo root)
- **Inno Setup 6+** — download from <https://jrsoftware.org/isinfo.php>
  - The `iscc.exe` command-line compiler must be on your `PATH`, or invoke it
    with its full path (default: `C:\Program Files (x86)\Inno Setup 6\iscc.exe`)
- Pre-built Rust release binaries (see Build steps below)

## Build Steps

### 1. Compile the Rust binaries

```powershell
# From the repository root
cargo build --release

# Copy the release binaries to the dist\ staging directory
mkdir dist
copy target\release\backup-agent-service.exe dist\
copy target\release\backup-agent-gui.exe      dist\
```

### 2. Compile the installer

```powershell
# From the repository root
iscc installer\setup.iss
```

The compiled installer will be written to:

```
installer\Output\BackupAgentSetup.exe
```

### 3. Run the installer

Execute `BackupAgentSetup.exe` on a Windows 10+ machine with an administrator
account. The installer will:

1. Copy both binaries to `%ProgramFiles%\BackupAgent\`
2. Create a desktop shortcut for the GUI
3. Register `backup-agent-service.exe` as a Windows Service (`BackupAgent`)

## Upgrade Behavior

Re-running the installer on a machine with an existing installation will:

1. Detect the prior installation via the Windows Service registry key
2. Stop the `BackupAgent` service before replacing any files
3. Copy the new binaries and restart the service

If the service cannot be stopped within ~10 seconds, the installer aborts
without modifying any files on disk to avoid a partially upgraded state.

## Uninstall

Use **Add or Remove Programs** in Windows Settings, or run the uninstaller
from `%ProgramFiles%\BackupAgent\`. The uninstaller will:

1. Stop the `BackupAgent` service
2. Run `backup-agent-service.exe uninstall` to deregister it from the SCM
3. Remove both binaries and all installer-managed files

## AppId GUID Note

The `AppId` GUID in `setup.iss` (`B7C2A4E1-3F8D-4C92-A1B5-9D6E0F2C7A34`) must
**never be changed** after the first public release. Changing it breaks upgrade
detection — Windows treats a different GUID as a new product, leaving the old
installation orphaned in Add/Remove Programs.

## Troubleshooting

- **GUI does nothing when launched: no window, no error dialog, no
  `Application` event log entry, process exits almost instantly.** This is
  the signature of `egui_glow` failing to get an OpenGL 2.0+ context — common
  on physical servers, since their onboard/BMC video chip (Matrox, ASPEED,
  etc.) typically only exposes OpenGL 1.1 through Windows' generic display
  driver. Running the GUI from `cmd.exe` with `tracing_subscriber` logging
  confirms it:
  ```
  ERROR eframe::native::run: Exiting because of error: egui_glow: OpenGL: egui_glow requires opengl 2.0+.
  ```
  Fix: drop a software OpenGL implementation next to the GUI executable.
  Windows loads a DLL from the application's own directory before falling
  back to `System32`, so this doesn't touch the system-wide driver.
  1. Get `opengl32.dll` from a prebuilt Mesa3D-for-Windows package — e.g.
     [fdossena.com/Mesa3D](https://fdossena.com/?p=mesa%2Findex.frag) or
     [pal1000/mesa-dist-win](https://github.com/pal1000/mesa-dist-win).
  2. Copy it into `%ProgramFiles(x86)%\BackupAgent\` (next to
     `backup-agent-gui.exe`).
  3. Relaunch the GUI. It now renders via CPU software rasterization
     (llvmpipe), which is plenty for this UI's complexity — no noticeable
     slowdown.

  As of this branch, `main.rs` also shows a native message box with these
  same instructions if `eframe::run_native` fails, instead of exiting
  silently (see `crates/gui/src/main.rs`).

## Known Limitations

- **`log_rotate_threshold_bytes` has no effect on actual rotation threshold.**
  The `config.toml` field `[service] log_rotate_threshold_bytes` is validated
  and stored but the service always uses the hardcoded 10 MiB threshold at
  startup. This is because `config.toml` is not loaded until after log rotation
  runs in `main()`. Configurable threshold is deferred to a future release.

- **Single `.log.1` backup only.** Log rotation keeps only one previous log
  file (`backup-agent.log.1`). Older rotations are overwritten.

- **Rotation is startup-only.** If the log grows past 10 MiB while the service
  is running, no mid-run rotation occurs. The service must be restarted for
  rotation to trigger.

## Manual Smoke Test Checklist

After installing, verify the following manually:

- [ ] Both `backup-agent-service.exe` and `backup-agent-gui.exe` are present in
      the install directory (`%ProgramFiles%\BackupAgent\`)
- [ ] `sc query BackupAgent` reports the service as `RUNNING`
- [ ] Desktop shortcut opens the GUI without errors
- [ ] Upgrade: run a newer installer → service stops, files replace, service
      restarts; SCM shows the updated binary path
- [ ] Uninstall: `sc query BackupAgent` returns `FAILED 1060` (service not
      found) and the install directory is removed
- [ ] Dashboard combobox: open GUI → select a database → resize window → same
      database remains selected (no `unsafe` global state regression)
- [ ] Settings monthly time: open a Monthly schedule task → change the time
      field → save → reload config → `monthly_time` updated, `weekly_time`
      unchanged
- [ ] Log rotation: write 10 MiB+ to `backup-agent.log` → restart service →
      `backup-agent.log.1` exists and new `backup-agent.log` is small
- [ ] `grep -r "unsafe" crates/gui/` returns zero matches
