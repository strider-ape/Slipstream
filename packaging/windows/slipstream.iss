; Slipstream installer for Windows (Inno Setup 6).
; Per-user install: no administrator rights needed.
; Build: iscc /DVersion=0.2.0 /DBin=<folder with the .exe files> /DOut=<output folder> slipstream.iss

#ifndef Version
  #define Version "0.0.0"
#endif
#ifndef Bin
  #define Bin "..\..\target\release"
#endif
#ifndef Out
  #define Out "..\..\dist"
#endif

[Setup]
AppId={{8C2B5E1A-6E7F-4F5B-9B1E-3C2D1A0F5E77}
AppName=Slipstream
AppVersion={#Version}
AppPublisher=Slipstream contributors
AppPublisherURL=https://github.com/strider-ape/Slipstream
AppSupportURL=https://github.com/strider-ape/Slipstream/issues
DefaultDirName={localappdata}\Programs\Slipstream
PrivilegesRequired=lowest
DisableProgramGroupPage=yes
DisableDirPage=yes
LicenseFile=..\..\LICENSE
OutputDir={#Out}
OutputBaseFilename=Slipstream-Setup-{#Version}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=....ssetsslipstream.ico
UninstallDisplayIcon={app}\slipstream-gui.exe
UninstallDisplayName=Slipstream
; Close running Slipstream programs before replacing them.
CloseApplications=force

[Tasks]
Name: "autostart"; Description: "Start Slipstream when I log in (sits in the tray)"
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#Bin}\slipstream.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Bin}\slipstream-daemon.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Bin}\slipstream-nmhost.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Bin}\slipstream-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Slipstream"; Filename: "{app}\slipstream-gui.exe"
Name: "{autodesktop}\Slipstream"; Filename: "{app}\slipstream-gui.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Slipstream"; ValueData: """{app}\slipstream-gui.exe"""; Tasks: autostart; Flags: uninsdeletevalue

[Run]
; Tell Chrome, Edge, Brave, Vivaldi, Chromium and Firefox where the browser bridge is.
Filename: "{app}\slipstream.exe"; Parameters: "register-browsers"; Flags: runhidden
Filename: "{app}\slipstream-gui.exe"; Description: "Open Slipstream now"; Flags: postinstall nowait skipifsilent

[UninstallRun]
Filename: "{app}\slipstream.exe"; Parameters: "daemon stop"; Flags: runhidden; RunOnceId: "StopDaemon"
Filename: "{sys}\taskkill.exe"; Parameters: "/im slipstream-gui.exe /f"; Flags: runhidden; RunOnceId: "StopGui"
Filename: "{app}\slipstream.exe"; Parameters: "unregister-browsers"; Flags: runhidden; RunOnceId: "Unregister"
