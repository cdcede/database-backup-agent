; =============================================================================
; Backup Agent — Inno Setup 6 installer script
;
; Prerequisites:
;   - Inno Setup 6+ (https://jrsoftware.org/isinfo.php)
;   - Pre-built binaries in ..\dist\
;       backup-agent-service.exe
;       backup-agent-gui.exe
;
; Build command (from repo root):
;   iscc installer\setup.iss
;
; Output: installer\Output\BackupAgentSetup.exe
;
; IMPORTANT — AppId GUID:
;   The GUID below identifies this product in the Windows registry.
;   Do NOT regenerate it. Changing the GUID breaks upgrade detection and
;   will result in duplicate Start Menu entries and registry pollution.
; =============================================================================

#define AppName    "Backup Agent"
#define AppVersion "1.0.0"
#define AppPublisher "Backup Agent Project"
#define ServiceExe "backup-agent-service.exe"
#define GuiExe     "backup-agent-gui.exe"

[Setup]
; DO NOT change this GUID — it identifies the product for upgrade detection
AppId={{B7C2A4E1-3F8D-4C92-A1B5-9D6E0F2C7A34}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppVerName={#AppName} {#AppVersion}

; Installation directory — {autopf} resolves to Program Files on 32/64-bit
DefaultDirName={autopf}\BackupAgent
DefaultGroupName={#AppName}

; Output installer binary
OutputDir=Output
OutputBaseFilename=BackupAgentSetup

; Require administrator privileges (needed for Windows Service registration)
PrivilegesRequired=admin

; Compression
Compression=lzma2
SolidCompression=yes

; Minimum OS: Windows 7 / Server 2008 R2 (NT 6.1). This branch is built with
; a Rust 1.77.2 toolchain (see rust-toolchain.toml) specifically to support
; Windows Server 2012 R2 (NT 6.3), which is below the Windows 10 floor that
; Rust 1.78+ requires. See installer/README.md for the Visual C++ Redistributable
; prerequisite this build needs on pre-Windows-10 targets.
MinVersion=6.1sp1

; Show wizard pages
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
; Service binary — ignoreversion: always overwrite even if version unchanged
Source: "..\dist\{#ServiceExe}"; DestDir: "{app}"; Flags: ignoreversion
; GUI binary
Source: "..\dist\{#GuiExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; Desktop shortcut for the GUI
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#GuiExe}"
; Start Menu entry
Name: "{group}\{#AppName}"; Filename: "{app}\{#GuiExe}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"

[Run]
; Register the Windows Service (fresh install). On upgrade the service entry
; already exists so sc.exe create will exit non-zero — that is expected and
; harmless; the binary has already been replaced and the next step starts it.
Filename: "{app}\{#ServiceExe}"; Parameters: "install"; \
    Flags: runhidden waituntilterminated runascurrentuser; \
    StatusMsg: "Registering Windows Service..."
; Start the service — covers both fresh install (just registered above) and
; upgrade (service was stopped in PrepareToInstall; entry already existed).
Filename: "net.exe"; Parameters: "start BackupAgent"; \
    Flags: runhidden waituntilterminated runascurrentuser; \
    StatusMsg: "Starting Backup Agent Service..."

[UninstallRun]
; Stop the service before the uninstaller removes files.
; RunOnceId ensures this runs exactly once even if the uninstaller retries.
Filename: "net.exe"; Parameters: "stop BackupAgent"; \
    Flags: runhidden waituntilterminated; \
    RunOnceId: "StopBackupAgentSvc"
; Deregister the service.
Filename: "{app}\{#ServiceExe}"; Parameters: "uninstall"; \
    Flags: runhidden waituntilterminated; \
    RunOnceId: "UnregBackupAgentSvc"

[Code]
// ---------------------------------------------------------------------------
// InitializeSetup — called before the wizard UI is shown.
//
// This build targets Windows 7 / Server 2008 R2 and up (see MinVersion),
// including Windows Server 2012 R2. On those pre-Windows-10 OSes, the
// binaries need the Visual C++ 2015-2022 Redistributable (x64) installed —
// it provides the Universal C Runtime (api-ms-win-crt-*.dll) that a
// Rust/MSVC build links against, which ships built into Windows 10+ but not
// into older OSes. Warn (non-blocking) if it looks absent.
// ---------------------------------------------------------------------------

function VCRedistInstalled: Boolean;
begin
  Result := RegKeyExists(HKLM, 'SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\X64') or
            RegKeyExists(HKLM, 'SOFTWARE\WOW6432Node\Microsoft\VisualStudio\14.0\VC\Runtimes\X64');
end;

function InitializeSetup: Boolean;
begin
  Result := True;
  if not VCRedistInstalled then
  begin
    if MsgBox('No se detectó el Visual C++ Redistributable 2015-2022 (x64), requerido por Backup Agent en este sistema operativo.' + #13#10 + #13#10 +
              'Descárgalo e instálalo desde: https://aka.ms/vs/17/release/vc_redist.x64.exe' + #13#10 + #13#10 +
              '¿Continuar con la instalación de todos modos?',
              mbConfirmation, MB_YESNO) = IDNO then
      Result := False;
  end;
end;

var
  ResultCode: Integer;

function ServiceIsRunning: Boolean;
var
  TempFile: String;
  Output: String;
begin
  // Query the SCM; exit code 0 means the service entry exists and is running.
  // We treat any non-zero exit as "not running" (stopped, not installed, etc.)
  Result := Exec('sc.exe', 'query BackupAgent', '', SW_HIDE,
                 ewWaitUntilTerminated, ResultCode) and (ResultCode = 0);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  StopResultCode: Integer;
  Retries: Integer;
begin
  Result := '';

  // Only act if a prior installation exists (upgrade path).
  if RegKeyExists(HKLM, 'SYSTEM\CurrentControlSet\Services\BackupAgent') then
  begin
    if ServiceIsRunning then
    begin
      // Attempt to stop the service; give it up to ~10 seconds.
      Exec('net.exe', 'stop BackupAgent', '', SW_HIDE,
           ewWaitUntilTerminated, StopResultCode);

      // Poll until stopped or timeout (5 × 2s = 10s).
      Retries := 5;
      while ServiceIsRunning and (Retries > 0) do
      begin
        Sleep(2000);
        Retries := Retries - 1;
      end;

      if ServiceIsRunning then
      begin
        // Service failed to stop — abort to protect existing installation.
        Result := 'Could not stop the Backup Agent service before upgrade. ' +
                  'Please stop the service manually and run the installer again.';
        Exit;
      end;
    end;
  end;
end;

// ---------------------------------------------------------------------------
// CurUninstallStepChanged — called at each stage of the uninstall process.
//
// We stop the service at usUninstall (before files are deleted) so the
// uninstaller can remove the locked service binary.
// ---------------------------------------------------------------------------

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    // Stop the service; ignore errors (may already be stopped).
    Exec('net.exe', 'stop BackupAgent', '', SW_HIDE,
         ewWaitUntilTerminated, ResultCode);
    Sleep(2000);
  end;
end;
