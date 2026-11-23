; OxideStudio (OBS-Rust) Windows x64 Installer Script (Inno Setup 6)
#define AppName "OxideStudio (OBS-Rust)"
#define AppVersion "0.1.1"
#define AppPublisher "OxideStudio Contributors"
#define AppURL "https://github.com/whiskyyyyys/Screen-Recorder"
#define AppExeName "obs-rust.exe"

[Setup]
AppId={{8F9D4A1C-4B32-4C7D-9F63-31A7A1C4E801}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
DefaultDirName={autopf}\OxideStudio
DefaultGroupName={#AppName}
AllowNoIcons=yes
LicenseFile=..\..\LICENSE
OutputDir=..\..\dist
OutputBaseFilename=OBS-Rust-Setup-x64
SetupIconFile=..\..\assets\icons\obs-rust.ico
Compression=lzma2/normal
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequiredOverridesAllowed=dialog
UninstallDisplayIcon={app}\{#AppExeName}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\..\dist\OBS-Rust-Portable-x64\obs-rust.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\OBS-Rust-Portable-x64\ffmpeg.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\OBS-Rust-Portable-x64\ffprobe.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\OBS-Rust-Portable-x64\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\OBS-Rust-Portable-x64\THIRD_PARTY_LICENSES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\OBS-Rust-Portable-x64\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\OBS-Rust-Portable-x64\plugins\*"; DestDir: "{app}\plugins"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\..\dist\OBS-Rust-Portable-x64\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(AppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent
