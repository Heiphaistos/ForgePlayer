; Inno Setup script — ForgePlayer Windows installer
; Build: ISCC.exe installer\ForgePlayer.iss (run from repo root, expects dist\ populated by build.bat release x64)

#define MyAppName "ForgePlayer"
#define MyAppVersion "1.5.0"
#define MyAppPublisher "ForgePlayer"
#define MyAppExeName "launch.bat"
#define MyDistDir "..\dist"

[Setup]
; AppId inchangé volontairement (renommage in-place — un ancien install
; OmniPlayer se met à jour proprement au lieu de dupliquer l'entrée désinstalleur).
AppId={{9E6B0C1A-9F0B-4C7B-8E9B-3D6E8F2F1A11}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
OutputDir=..\dist
OutputBaseFilename=ForgePlayer_v{#MyAppVersion}_Setup
SetupIconFile=..\crates\forgeplayer\assets\icon.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\ForgePlayer.exe
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "french"; MessagesFile: "compiler:Languages\French.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#MyDistDir}\ForgePlayer.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#MyDistDir}\subtitle-service.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#MyDistDir}\media-indexer.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#MyDistDir}\*.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#MyDistDir}\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "launch_installed.bat"; DestDir: "{app}"; DestName: "launch.bat"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"; IconFilename: "{app}\ForgePlayer.exe"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"; IconFilename: "{app}\ForgePlayer.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent runasoriginaluser
