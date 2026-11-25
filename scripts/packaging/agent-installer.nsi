; Lunaris Agent installer for Windows.
; Built in CI from the repository root:
;   makensis /DVERSION=<x.y.z> scripts\packaging\agent-installer.nsi
; expecting the command-line agent (lunaris-agent.exe, FFmpeg and MSVC runtime
; DLLs) in .\lunaris-agent\ and the approval helper (windeployqt'ed) in
; .\lunaris-approval-helper\.
;
; The agent has no window: it runs as the LunarisAgentService Windows service
; (as SYSTEM in the console session, agent/src/win_service.rs), installed and
; started here, and is managed from the Lunaris server's web page. Join a
; server with the one-line installer of the server's web page, or from an
; elevated prompt:
;   $env:LUNARIS_INSTALL_TOKEN='lit_...'; & "<install folder>\lunaris-agent.exe" --system --enroll --server https://SERVER
;   Restart-Service LunarisAgentService
; The optional component "Approval helper" (lunaris-approval-helper.exe, Qt)
; lets the person at the computer accept connections made without the
; password.

!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif

!define APP     "Lunaris Agent"
!define EXE     "lunaris-agent.exe"
!define UNINST  "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}"

Name "${APP}"
; One LZMA stream over all files: about a third smaller than the default.
SetCompressor /SOLID lzma
OutFile "..\..\lunaris-agent-installer.exe"
InstallDir "$PROGRAMFILES64\${APP}"
InstallDirRegKey HKLM "Software\${APP}" "InstallDir"
RequestExecutionLevel admin
Unicode true

!define MUI_ABORTWARNING
!define MUI_ICON   "..\..\agent\icons\icon.ico"
!define MUI_UNICON "..\..\agent\icons\icon.ico"
!define MUI_FINISHPAGE_TEXT "Lunaris Agent is installed and runs as the $\"Lunaris Agent$\" Windows service (it starts at boot).$\r$\n$\r$\nJoin a Lunaris server with the install command shown on the server's web page (Computers, Add), or from an elevated prompt: lunaris-agent.exe --system --enroll --server <address> (install token in LUNARIS_INSTALL_TOKEN), then restart the service.$\r$\n$\r$\nlunaris-agent.exe --system status shows its state. Settings, log: %ProgramData%\Lunaris."

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\..\LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"


; The agent's Windows service (LunarisAgentService, `--install-service`, see
; agent/src/win_service.rs) runs the program from its install folder, which
; keeps the files in use. An install (also the app's silent update, `/S`)
; stops it before copying and starts it again afterwards when it was running.
Var ServiceWasRunning
Var ServiceExisted
Var HelperChosen

; ---- Safe install / uninstall -------------------------------------------------
; The uninstaller removes only what the installer put there (install.log), never a
; whole folder, so a mistaken install folder cannot lose the user's own files.
Var LogFile

; An existing install folder must be empty or a previous install of this app
; (uninstaller + main exe). Result in $InstDirOk.
Var InstDirOk
Function CheckInstDir
  Push $0
  Push $1
  StrCpy $InstDirOk 1
  IfFileExists "$INSTDIR\*.*" 0 cid_done
  IfFileExists "$INSTDIR\uninstall.exe" 0 cid_scan
  IfFileExists "$INSTDIR\${EXE}" cid_done cid_scan
  cid_scan:
  FindFirst $0 $1 "$INSTDIR\*.*"
  cid_loop:
    StrCmp $1 "" cid_empty
    StrCmp $1 "." cid_next
    StrCmp $1 ".." cid_next
    FindClose $0
    StrCpy $InstDirOk 0
    Goto cid_done
  cid_next:
    FindNext $0 $1
    Goto cid_loop
  cid_empty:
    FindClose $0
  cid_done:
  Pop $1
  Pop $0
FunctionEnd

Function .onVerifyInstDir
  Call CheckInstDir
  StrCmp $InstDirOk 1 +2
    Abort
FunctionEnd

; Writes every file ("F|rel") and folder ("D|rel", after its contents) under
; $INSTDIR\<rel> to the log. Argument on the stack: rel ("" for the root).
Function LogDir
  Exch $R0
  Push $R1
  Push $R2
  Push $R3
  Push $R4
  StrCpy $R3 "$INSTDIR"
  StrCmp $R0 "" +2
    StrCpy $R3 "$INSTDIR\$R0"
  FindFirst $R1 $R2 "$R3\*.*"
  ld_loop:
    StrCmp $R2 "" ld_end
    StrCmp $R2 "." ld_next
    StrCmp $R2 ".." ld_next
    StrCpy $R4 "$R2"
    StrCmp $R0 "" +2
      StrCpy $R4 "$R0\$R2"
    IfFileExists "$R3\$R2\*.*" ld_dir ld_file
  ld_dir:
    Push $R4
    Call LogDir
    Goto ld_next
  ld_file:
    StrCmp $R4 "install.log" ld_next
    StrCmp $R4 "uninstall.exe" ld_next
    FileWrite $LogFile "F|$R4$\n"
  ld_next:
    FindNext $R1 $R2
    Goto ld_loop
  ld_end:
  FindClose $R1
  StrCmp $R0 "" +2
    FileWrite $LogFile "D|$R0$\n"
  Pop $R4
  Pop $R3
  Pop $R2
  Pop $R1
  Pop $R0
FunctionEnd

; 1 on the stack if the log path is unsafe (absolute, drive, or ..), else 0.
Function un.BadRel
  Exch $R0
  Push $R1
  Push $R2
  StrCpy $R1 0
  StrCpy $R2 $R0 1
  StrCmp $R2 "" br_bad
  StrCmp $R2 "\" br_bad
  StrCmp $R2 "/" br_bad
  br_loop:
    StrCpy $R2 $R0 2 $R1
    StrCmp $R2 ".." br_bad
    StrCpy $R2 $R0 1 $R1
    StrCmp $R2 "" br_good
    StrCmp $R2 ":" br_bad
    IntOp $R1 $R1 + 1
    Goto br_loop
  br_bad:
    StrCpy $R0 1
    Goto br_end
  br_good:
    StrCpy $R0 0
  br_end:
  Pop $R2
  Pop $R1
  Exch $R0
FunctionEnd

Section "Lunaris Agent (service)" SecAgent
  SectionIn RO
  Call CheckInstDir
  StrCmp $InstDirOk 1 +3
    MessageBox MB_OK|MB_ICONSTOP "The install folder $INSTDIR is not empty and is not a previous install. Choose an empty folder." /SD IDOK
    Abort
  ; Stop the agent service (if installed and running) so its files can be replaced.
  StrCpy $ServiceWasRunning 0
  StrCpy $ServiceExisted 0
  StrCpy $HelperChosen 0
  nsExec::Exec 'sc query LunarisAgentService'
  Pop $0
  StrCmp $0 0 0 +2
    StrCpy $ServiceExisted 1
  nsExec::Exec 'net stop LunarisAgentService'
  Pop $0
  StrCmp $0 0 0 svc_not_running
    StrCpy $ServiceWasRunning 1
  svc_not_running:
  ; Replace a running copy on upgrade.
  nsExec::Exec 'taskkill /IM "${EXE}" /F'
  nsExec::Exec 'taskkill /IM "lunaris-approval-helper.exe" /F'

  SetOutPath "$INSTDIR"
  File /r "..\..\lunaris-agent\*"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  ; The service runs this folder's exe as SYSTEM and the approval channel
  ; trusts its folder: only SYSTEM and Administrators may change it, even
  ; when a custom folder was chosen (/D=).
  nsExec::ExecToLog 'icacls "$INSTDIR" /inheritance:r /grant:r *S-1-5-18:(OI)(CI)F *S-1-5-32-544:(OI)(CI)F *S-1-5-32-545:(OI)(CI)RX'
  Pop $0
  StrCmp $0 0 +2 0
    Abort "Could not restrict access to $INSTDIR"
  WriteRegStr HKLM "Software\${APP}" "InstallDir" "$INSTDIR"

  ; The agent window is gone: its shortcuts and its "Start at login" entry
  ; (HKCU Run `--minimized`, this user's) of earlier versions go too.
  Delete "$SMPROGRAMS\${APP}\${APP}.lnk"
  Delete "$DESKTOP\${APP}.lnk"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "LunarisAgent"
  CreateDirectory "$SMPROGRAMS\${APP}"
  CreateShortcut "$SMPROGRAMS\${APP}\Uninstall ${APP}.lnk" "$INSTDIR\uninstall.exe"
  ; Set again by the helper component when it is chosen.
  DeleteRegValue HKLM "Software\Microsoft\Windows\CurrentVersion\Run" "LunarisAgentApproval"

  ; Let peers reach the agent directly (WebRTC P2P / WebTransport).
  nsExec::Exec 'netsh advfirewall firewall delete rule name="${APP}"'
  nsExec::Exec 'netsh advfirewall firewall add rule name="${APP}" dir=in action=allow program="$INSTDIR\${EXE}" enable=yes'
SectionEnd

; The approval helper (optional, lunaris-approval-helper.exe beside the agent;
; approval-helper/): the agent runs as the Windows service (SYSTEM, no
; window), so this tray asks the person at the computer to accept connections
; made without the password (approval-channel/). The agent only lets a helper
; from its own folder answer. Started at logon for every user by the
; machine-wide Run key, unelevated in each user's own session; users can turn
; it off in Task Manager's Startup apps. Without it only the password lets
; anyone in.
Section "Approval helper (accept connections on this screen)" SecHelper
  SetOutPath "$INSTDIR"
  File /r "..\..\lunaris-approval-helper\*"
  StrCpy $HelperChosen 1
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Run" "LunarisAgentApproval" '"$INSTDIR\lunaris-approval-helper.exe"'
SectionEnd

Section "-Service and records"
  ; Record what was installed so the uninstaller removes exactly that.
  FileOpen $LogFile "$INSTDIR\install.log" w
  Push ""
  Call LogDir
  FileClose $LogFile

  WriteRegStr HKLM "${UNINST}" "DisplayName" "${APP}"
  WriteRegStr HKLM "${UNINST}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST}" "Publisher" "Lunaris"
  WriteRegStr HKLM "${UNINST}" "DisplayIcon" '"$INSTDIR\${EXE}"'
  WriteRegStr HKLM "${UNINST}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "${UNINST}" "URLInfoAbout" "https://github.com/collyn/lunaris"
  WriteRegDWORD HKLM "${UNINST}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST}" "NoRepair" 1

  ; The service: created on a first install (start at boot, started now), on
  ; an upgrade only started again when it was running (an administrator may
  ; have stopped it on purpose).
  StrCmp $ServiceExisted 1 svc_upgrade
    ; The settings of the agent window of earlier versions (this user's
    ; %APPDATA%) become the service's, when it has none yet.
    StrCpy $1 0
    ReadEnvStr $2 ProgramData
    IfFileExists "$2\Lunaris\agent_config.json" +3
    IfFileExists "$APPDATA\lunaris-agent\agent_config.json" 0 +2
      StrCpy $1 1
    nsExec::ExecToLog '"$INSTDIR\${EXE}" --install-service'
    Pop $0
    StrCmp $0 0 +2
      MessageBox MB_OK|MB_ICONEXCLAMATION "The Lunaris Agent service could not be installed (exit code $0). Run $\"$INSTDIR\${EXE}$\" --install-service from an elevated prompt." /SD IDOK
    StrCmp $1 1 0 svc_done
      ; That file is the user's own and any program of the user could have
      ; written it (another server address for the SYSTEM service): it is
      ; taken only when it is a plain file owned by this user (or by
      ; Administrators: earlier agents ran elevated), the server it names is
      ; shown in the details, and an interactive install asks first. The
      ; path goes through the environment (no quoting problems).
      System::Call 'Kernel32::SetEnvironmentVariable(t "LUNARIS_IMPORT_CONFIG", t "$APPDATA\lunaris-agent\agent_config.json")i'
      nsExec::ExecToStack 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$ErrorActionPreference = $\'Stop$\'; $$p = $$env:LUNARIS_IMPORT_CONFIG; if ((Get-Item -LiteralPath $$p -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { $\'a link$\'; exit 2 }; $$o = (Get-Acl -LiteralPath $$p).GetOwner([Security.Principal.SecurityIdentifier]).Value; $$me = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value; if ($$o -ne $$me -and $$o -ne $\'S-1-5-32-544$\') { $\'owned by $\' + $$o; exit 3 }; $$u = (Get-Content -LiteralPath $$p -Raw | ConvertFrom-Json).server_url; if (-not $$u) { $$u = $\'(none)$\' }; [string]$$u"'
      Pop $0
      Pop $3
      StrCmp $0 0 imp_check_ok
        DetailPrint "Not importing $APPDATA\lunaris-agent\agent_config.json: $3"
        Goto svc_done
      imp_check_ok:
      DetailPrint "Settings of this user's earlier Lunaris Agent: $APPDATA\lunaris-agent\agent_config.json, server $3"
      IfSilent imp_do
      MessageBox MB_YESNO|MB_ICONQUESTION "This user ran an earlier Lunaris Agent. Use its settings for the agent service?$\r$\n$\r$\nServer: $3$\r$\n$\r$\nChoose No unless you know this server. The agent can join a server later from the server's web page." /SD IDYES IDYES imp_do
        DetailPrint "Earlier settings not imported (declined)"
        Goto svc_done
      imp_do:
      nsExec::ExecToLog '"$INSTDIR\${EXE}" --system --import-config "$APPDATA\lunaris-agent\agent_config.json" status'
      Pop $0
      nsExec::Exec 'net stop LunarisAgentService'
      Pop $0
      nsExec::Exec 'net start LunarisAgentService'
      Pop $0
    Goto svc_done
  svc_upgrade:
  StrCmp $ServiceWasRunning 1 0 svc_done
    nsExec::Exec 'net start LunarisAgentService'
    Pop $0
  svc_done:

  ; The helper for the person installing, now (unelevated, through Explorer);
  ; everyone else gets it at their next logon.
  IfSilent +3
  StrCmp $HelperChosen 1 0 +2
    Exec '"$WINDIR\explorer.exe" "$INSTDIR\lunaris-approval-helper.exe"'
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecAgent} "The agent (command line, no window), run by the Lunaris Agent Windows service."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecHelper} "A tray program that asks the person at this computer before anyone connects without the password (needs Qt, about 40 MB)."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  ; The service first (it runs the program from this folder).
  nsExec::ExecToLog '"$INSTDIR\${EXE}" --uninstall-service'
  nsExec::Exec 'taskkill /IM "${EXE}" /F'
  nsExec::Exec 'taskkill /IM "lunaris-approval-helper.exe" /F'
  ; "Start at login" entry of the agent window of earlier versions (current user).
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "LunarisAgent"
  DeleteRegValue HKLM "Software\Microsoft\Windows\CurrentVersion\Run" "LunarisAgentApproval"
  nsExec::Exec 'netsh advfirewall firewall delete rule name="${APP}"'

  Delete "$SMPROGRAMS\${APP}\${APP}.lnk"
  Delete "$SMPROGRAMS\${APP}\Uninstall ${APP}.lnk"
  RMDir "$SMPROGRAMS\${APP}"
  Delete "$DESKTOP\${APP}.lnk"

  DeleteRegKey HKLM "${UNINST}"
  DeleteRegKey HKLM "Software\${APP}"
  ; Remove what the installer recorded, then the folder only if it is now empty.
  IfFileExists "$INSTDIR\install.log" 0 un_nolog
  FileOpen $0 "$INSTDIR\install.log" r
  un_loop:
    FileRead $0 $1
    StrCmp $1 "" un_done
  un_trim:
    StrCpy $2 $1 1 -1
    StrCmp $2 "$\n" un_cut
    StrCmp $2 "$\r" un_cut
    Goto un_trimmed
  un_cut:
    StrCpy $1 $1 -1
    Goto un_trim
  un_trimmed:
    StrCpy $2 $1 2
    StrCpy $3 $1 "" 2
    Push $3
    Call un.BadRel
    Pop $4
    StrCmp $4 1 un_loop
    StrCmp $2 "F|" un_file
    StrCmp $2 "D|" un_dir
    Goto un_loop
  un_file:
    Delete "$INSTDIR\$3"
    Goto un_loop
  un_dir:
    RMDir "$INSTDIR\$3"
    Goto un_loop
  un_done:
  FileClose $0
  Delete "$INSTDIR\install.log"
  un_nolog:
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  ; Settings in %ProgramData%\Lunaris (the service's) and
  ; %APPDATA%\lunaris-agent are kept.
SectionEnd
