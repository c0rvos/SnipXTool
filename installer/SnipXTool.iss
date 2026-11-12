; SnipXTool installer (Inno Setup 6)
; Build with: scripts\build-release.ps1
; Version can be overridden from the command line: ISCC.exe /DAppVersion=1.2.3 SnipXTool.iss

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif

#define AppName "SnipXTool"
#define AppPublisher "SnipXTool"
#define AppExeName "snipxtool.exe"

[Setup]
; Keep this GUID stable: it is how Windows recognises upgrades/uninstalls.
AppId={{A081C6AD-FE8B-4E98-8CCD-BDC62CFDF607}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
VersionInfoVersion={#AppVersion}
VersionInfoProductVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup
VersionInfoCompany={#AppPublisher}
DefaultDirName={autopf}\SnipXTool
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
OutputDir=..\dist
OutputBaseFilename=SnipXTool-{#AppVersion}-Setup
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExeName}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes
RestartApplications=no
AllowNoIcons=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
; Optional: ffmpeg.exe staged by build-release.ps1 (bundled by default).
Source: "..\build\ffmpeg\ffmpeg.exe"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\build\ffmpeg\README.txt"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\assets\logo.png"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; User-facing shortcuts open the window explicitly...
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Parameters: "--show"; IconFilename: "{app}\{#AppExeName}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Parameters: "--show"; IconFilename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Registry]
; "Start with Windows" is managed inside the app (HKCU Run value "SnipXTool",
; written with the installed exe path). Clear the value on install so it is
; re-created pointing at this build, and again on uninstall. Values written by
; previous builds ("SnipX", "RustSnip") are legacy names and are removed too.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "SnipXTool"; Flags: deletevalue uninsdeletevalue
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "SnipX"; Flags: deletevalue uninsdeletevalue
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "RustSnip"; Flags: deletevalue uninsdeletevalue

[Run]
Filename: "{app}\{#AppExeName}"; Parameters: "--show"; Description: "{cm:LaunchProgram,{#StringChange(AppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[Code]
function FfmpegAvailable(): Boolean;
var
  ResultCode: Integer;
begin
  Result := FileExists(ExpandConstant('{app}\ffmpeg.exe'));
  if not Result then
  begin
    Result :=
      Exec(ExpandConstant('{cmd}'), '/c where ffmpeg >nul 2>nul', '',
           SW_HIDE, ewWaitUntilTerminated, ResultCode) and (ResultCode = 0);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and (not WizardSilent) and (not FfmpegAvailable()) then
  begin
    MsgBox('ffmpeg was not found.' + #13#10#13#10 +
           'Snips and the editor work without it, but recording and video export need ffmpeg.' + #13#10 +
           'Install it with:  winget install Gyan.FFmpeg' + #13#10 +
           'or set the ffmpeg path in SnipXTool Settings.',
           mbInformation, MB_OK);
  end;
end;
