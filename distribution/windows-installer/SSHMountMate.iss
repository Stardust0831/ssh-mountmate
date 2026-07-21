; Per-user, fixed-path installer for the Windows installed edition.
; Build with: iscc /DARCH=x64|arm64 /DAPP_VERSION=... /DINPUT_EXE=... /DOUTPUT_DIR=...

#ifndef APP_VERSION
  #error APP_VERSION must be supplied by the release workflow
#endif
#ifndef INPUT_EXE
  #error INPUT_EXE must be supplied by the release workflow
#endif
#ifndef OUTPUT_DIR
  #define OUTPUT_DIR "output"
#endif
#ifndef ARCH
  #define ARCH "x64"
#endif

#if ARCH == "arm64"
  #define ARCH_ALLOWED "arm64"
  #define ARCH_MODE "arm64"
#else
  #define ARCH_ALLOWED "x64compatible"
  #define ARCH_MODE "x64compatible"
#endif

#define AppName "SSH MountMate"
#define AppExeName "SSHMountMate.exe"
#define InstallRoot "{localappdata}\Programs\SSH MountMate"
#define InstallRecord "Software\Stardust\SSH MountMate\Install"
#define Aumid "Stardust.SSHMountMate"

[Setup]
AppId={{5CCBBD52-BF64-4E48-9B41-6F3BF3C562A7}
AppName={#AppName}
AppVersion={#APP_VERSION}
AppPublisher=Stardust0831
DefaultDirName={#InstallRoot}
DisableDirPage=yes
DisableProgramGroupPage=yes
ArchitecturesAllowed={#ARCH_ALLOWED}
ArchitecturesInstallIn64BitMode={#ARCH_MODE}
PrivilegesRequired=lowest
UsePreviousAppDir=no
Uninstallable=yes
UninstallDisplayIcon={app}\{#AppExeName}
OutputDir={#OUTPUT_DIR}
OutputBaseFilename=SSHMountMate-windows-{#ARCH}-setup
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no

[Files]
Source: "{#INPUT_EXE}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\SSH MountMate"; Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"; AppUserModelID: "{#Aumid}"

; The installer, rather than the application, owns these fixed-path Explorer
; registrations. They are removed with the per-user uninstall entry.
[Registry]
Root: HKCU; Subkey: "{#InstallRecord}"; ValueType: none; Flags: uninsdeletekeyifempty
Root: HKCU; Subkey: "{#InstallRecord}"; ValueName: "SchemaVersion"; ValueType: dword; ValueData: "1"
Root: HKCU; Subkey: "{#InstallRecord}"; ValueName: "Version"; ValueType: string; ValueData: "{#APP_VERSION}"
Root: HKCU; Subkey: "{#InstallRecord}"; ValueName: "InstallRoot"; ValueType: string; ValueData: "{#InstallRoot}"
Root: HKCU; Subkey: "{#InstallRecord}"; ValueName: "ExecutablePath"; ValueType: string; ValueData: "{#InstallRoot}\{#AppExeName}"
Root: HKCU; Subkey: "{#InstallRecord}"; ValueName: "Aumid"; ValueType: string; ValueData: "{#Aumid}"
Root: HKCU; Subkey: "{#InstallRecord}"; ValueName: "Architecture"; ValueType: string; ValueData: "{#ARCH}"
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\{#Aumid}"; ValueName: "DisplayName"; ValueType: string; ValueData: "{#AppName}"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\{#Aumid}"; ValueName: "IconUri"; ValueType: string; ValueData: "{#InstallRoot}\{#AppExeName}"
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\SSHMountMate.Refresh"; ValueName: ""; ValueType: string; ValueData: "Refresh with {#AppName}"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\SSHMountMate.Refresh"; ValueName: "Icon"; ValueType: string; ValueData: "{#InstallRoot}\{#AppExeName}"
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\SSHMountMate.Refresh\command"; ValueName: ""; ValueType: string; ValueData: """{#InstallRoot}\{#AppExeName}"" --refresh-path ""%V\."""
Root: HKCU; Subkey: "Software\Classes\Directory\shell\SSHMountMate.Refresh"; ValueName: ""; ValueType: string; ValueData: "Refresh with {#AppName}"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Directory\shell\SSHMountMate.Refresh"; ValueName: "Icon"; ValueType: string; ValueData: "{#InstallRoot}\{#AppExeName}"
Root: HKCU; Subkey: "Software\Classes\Directory\shell\SSHMountMate.Refresh\command"; ValueName: ""; ValueType: string; ValueData: """{#InstallRoot}\{#AppExeName}"" --refresh-path ""%1\."""
Root: HKCU; Subkey: "Software\Classes\Drive\shell\SSHMountMate.Refresh"; ValueName: ""; ValueType: string; ValueData: "Refresh with {#AppName}"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Drive\shell\SSHMountMate.Refresh"; ValueName: "Icon"; ValueType: string; ValueData: "{#InstallRoot}\{#AppExeName}"
Root: HKCU; Subkey: "Software\Classes\Drive\shell\SSHMountMate.Refresh\command"; ValueName: ""; ValueType: string; ValueData: """{#InstallRoot}\{#AppExeName}"" --refresh-path ""%1\."""
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\SSHMountMate.Transfers"; ValueName: ""; ValueType: string; ValueData: "Open {#AppName} transfers"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\SSHMountMate.Transfers\command"; ValueName: ""; ValueType: string; ValueData: """{#InstallRoot}\{#AppExeName}"" --show-transfers"
Root: HKCU; Subkey: "Software\Classes\Directory\shell\SSHMountMate.Transfers"; ValueName: ""; ValueType: string; ValueData: "Open {#AppName} transfers"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Directory\shell\SSHMountMate.Transfers\command"; ValueName: ""; ValueType: string; ValueData: """{#InstallRoot}\{#AppExeName}"" --show-transfers"
Root: HKCU; Subkey: "Software\Classes\Drive\shell\SSHMountMate.Transfers"; ValueName: ""; ValueType: string; ValueData: "Open {#AppName} transfers"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Drive\shell\SSHMountMate.Transfers\command"; ValueName: ""; ValueType: string; ValueData: """{#InstallRoot}\{#AppExeName}"" --show-transfers"

[UninstallDelete]
; Never delete settings, cache, credentials, or application data outside {app}.
Type: filesandordirs; Name: "{app}"

[Code]
function InitializeSetup(): Boolean;
var
  ExistingExecutable: String;
  ExitCode: Integer;
begin
  Result := True;
  if not RegQueryStringValue(HKCU, '{#InstallRecord}', 'ExecutablePath', ExistingExecutable) then
    exit;
  if not FileExists(ExistingExecutable) then
    exit;
  if not Exec(ExistingExecutable, '--installer-check-version {#APP_VERSION}', '', SW_HIDE,
      ewWaitUntilTerminated, ExitCode) then begin
    MsgBox('Could not validate the installed SSH MountMate version. Installation was blocked.', mbError, MB_OK);
    Result := False;
    exit;
  end;
  if ExitCode <> 0 then begin
    MsgBox('A newer SSH MountMate version is already installed. Uninstall it or use a newer installer.', mbError, MB_OK);
    Result := False;
  end;
end;

function InitializeUninstall(): Boolean;
var
  ExitCode: Integer;
begin
  Result := True;
  if not FileExists(ExpandConstant('{app}\{#AppExeName}')) then exit;
  ; The app owns the active-mount check. A non-zero result blocks uninstall.
  if not Exec(ExpandConstant('{app}\{#AppExeName}'), '--installer-uninstall-preflight', '', SW_HIDE, ewWaitUntilTerminated, ExitCode) then begin
    MsgBox('Could not run the SSH MountMate uninstall preflight; uninstall was blocked.', mbError, MB_OK);
    Result := False;
    exit;
  end;
  if ExitCode <> 0 then begin
    MsgBox('SSH MountMate reports active mounts. Unmount them before uninstalling.', mbError, MB_OK);
    Result := False;
  end;
end;
