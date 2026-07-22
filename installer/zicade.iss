; Zicade Windows installer - Inno Setup 6 script.
;
; Produces a single setup .exe that installs zicade.exe, offers a per-user vs
; all-users scope choice, and two optional startup mechanisms. Compiled in CI
; by ISCC against the MSVC-built exe (see .github/workflows/release-please.yml).
;
; Two symbols are passed on the command line; both have local-build fallbacks:
;
;     iscc /DMyAppVersion=0.2.0 /DExeSource=C:\...\zicade.exe installer\zicade.iss
;
; Paths to sibling repo files (icon, license) are resolved relative to this
; script via {#SourcePath} so the build is location-independent.

#ifndef MyAppVersion
  #define MyAppVersion "0.0.0"
#endif

#ifndef ExeSource
  ; Local default: the MSVC release build, relative to the repo root.
  #define ExeSource SourcePath + "..\target\x86_64-pc-windows-msvc\release\zicade.exe"
#endif

#define MyAppName "Zicade"
#define MyAppExe "zicade.exe"
#define MyAppPublisher "Zicade"

[Setup]
; A stable AppId keys upgrades and the uninstall entry - never change it.
AppId={{E7B9A6C2-3D4F-4A1B-9C8E-1F2A3B4C5D6E}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\Zicade
DefaultGroupName=Zicade
UninstallDisplayIcon={app}\{#MyAppExe}
OutputBaseFilename=zicade-{#MyAppVersion}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
LicenseFile={#SourcePath}..\LICENSE
SetupIconFile={#SourcePath}..\apps\zicade\assets\zicade.ico
; x64-only tool: install into the native 64-bit locations, refuse other archs.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Default to a non-elevated per-user install; the wizard's scope page lets the
; user switch to all-users, which triggers UAC only then.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

[Tasks]
Name: "startonlogin"; Description: "Start Zicade when I sign in (runs in the notification-area tray)"; Flags: unchecked
; A machine service needs admin, so this task only appears for an all-users install.
Name: "runasservice"; Description: "Run Zicade as a Windows service (all users, starts at boot)"; Flags: unchecked; Check: IsAdminInstallMode

[Files]
Source: "{#ExeSource}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Zicade"; Filename: "{app}\{#MyAppExe}"; Comment: "Local forwarding HTTP/HTTPS proxy"

[Registry]
; Start on login: launch the bare exe (no subcommand) so Zicade's console
; heuristic runs it in tray mode. HKA maps to HKLM (all users) or HKCU
; (per-user) to match the install scope.
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Zicade"; ValueData: """{app}\{#MyAppExe}"""; Flags: uninsdeletevalue; Tasks: startonlogin

[Run]
; Register the auto-start service via the app's own tested SCM path. Runs only
; when the (admin-only) task is selected.
Filename: "{app}\{#MyAppExe}"; Parameters: "service install"; Flags: runhidden; Tasks: runasservice

[UninstallRun]
; Remove the service on uninstall. Gated to admin installs; a no-op (nonzero
; exit) when no service was registered is harmless and ignored.
Filename: "{app}\{#MyAppExe}"; Parameters: "service uninstall"; Flags: runhidden; RunOnceId: "RemoveZicadeService"; Check: IsAdminInstallMode
