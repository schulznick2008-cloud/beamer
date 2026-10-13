; Inno Setup Script for Beamer Windows Receiver
; Built by build_win_app.ps1 -Release. The version is read from the repo-root VERSION file, the one
; place it is set for both apps.

#define VersionFile FileOpen(AddBackslash(SourcePath) + "..\VERSION")
#define AppVersion Trim(FileRead(VersionFile))
#expr FileClose(VersionFile)
; A beta such as 1.5.0-beta.1 names itself in the shown version, but the file version resource
; takes four integers, so it carries the release number.
#define NumericVersion Copy(AppVersion, 1, Pos("-", AppVersion + "-") - 1)

[Setup]
AppId={{9A4E2B1F-4309-4E65-B86D-C7B9F4F64D01}
AppName=Beamer
AppVersion={#AppVersion}
VersionInfoVersion={#NumericVersion}.0
VersionInfoProductTextVersion={#AppVersion}
AppPublisher=Kalkman Code
DefaultDirName={localappdata}\Beamer
DisableProgramGroupPage=yes
OutputBaseFilename=Beamer-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
; The installer itself still runs per-user; Beamer.exe carries its own requireAdministrator
; manifest (see build.spec) so SendInput can reach elevated windows. Explorer honours that
; manifest automatically (UAC shield on the shortcuts), no change needed here for that part.
PrivilegesRequired=lowest
UninstallDisplayIcon={app}\Beamer.exe
; The single-instance mutex Beamer.exe holds. A running Beamer is elevated, so this per-user setup
; can neither close it nor replace its locked exe; setup asks for it to be quit from the tray first.
AppMutex=Beamer.Receiver
SetupIconFile=Beamer.ico
; Beamer.exe is x64. This refuses 32-bit Windows with the installer's own message; Windows 11 on
; ARM is allowed, as it runs x64 apps under emulation.
ArchitecturesAllowed=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
; Autostart is asked for, never assumed, so it starts unticked.
Name: "autostart"; Description: "Start Beamer elevated at sign-in without another UAC prompt"; GroupDescription: "Startup options:"; Flags: unchecked

[Files]
Source: "dist\Beamer.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; The same paths Install-Beamer.ps1 writes, so either installer replaces the other's shortcuts
; instead of adding a second Beamer to the Start Menu.
Name: "{autoprograms}\Beamer"; Filename: "{app}\Beamer.exe"

[InstallDelete]
; Remove pre-task autostart entries. A Startup shortcut launches the requireAdministrator exe
; through Explorer and raises UAC on every sign-in, even when the scheduled task is also present.
Type: files; Name: "{userstartup}\Beamer.lnk"

[Registry]
; The same legacy Run entry has the same UAC behaviour as the old Startup shortcut.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "Beamer"; Flags: deletevalue

[Run]
; CreateProcess refuses a requireAdministrator exe from this non-elevated setup (error 740);
; shellexec lets Windows raise the UAC prompt instead.
Filename: "{app}\Beamer.exe"; Description: "{cm:LaunchProgram,Beamer}"; Flags: nowait postinstall skipifsilent shellexec

[UninstallRun]
; The elevated app reads the interactive account from the session, then removes its task, the
; legacy global task and firewall rules without relying on the UAC administrator's identity.
Filename: "{app}\Beamer.exe"; Parameters: "--uninstall-cleanup"; Flags: runhidden shellexec waituntilterminated; Verb: runas; RunOnceId: "BeamerCleanup"

[Code]
function RunTaskOperation(const Filename, Params: String; var ResultCode: Integer): Boolean;
begin
  if IsAdmin then
    Result := Exec(Filename, Params, '', SW_HIDE, ewWaitUntilTerminated, ResultCode)
  else
    Result := ShellExec('runas', Filename, Params, '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

function TaskCommandFailure(const Action: String; const Started: Boolean; const ResultCode: Integer): String;
begin
  if (not Started) and (ResultCode = 1223) then
    Result := 'You cancelled the permission request.'
  else if not Started then
    Result := 'Windows could not start the task operation: ' + SysErrorMessage(ResultCode)
  else
    Result := 'Windows returned exit code ' + IntToStr(ResultCode) + '.';
  Result := Action + ' ' + Result;
end;

// Keep the old elevated starter until first run migrates it successfully. This also preserves
// autostart when a silent upgrade does not launch Beamer after replacing the executable.

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
  Started: Boolean;
  Failure: String;
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('autostart') then
  begin
    Started := RunTaskOperation(ExpandConstant('{app}\Beamer.exe'), '--enable-autostart', ResultCode);
    if (not Started) or (ResultCode <> 0) then
    begin
      Failure := TaskCommandFailure('Could not run Beamer sign-in setup.', Started, ResultCode);
      Log(Failure);
      RaiseException('Beamer was installed, but start at sign-in was not configured. It requires Beamer and the signed-in session to use the same administrator account. ' + Failure);
    end;
  end;
end;
