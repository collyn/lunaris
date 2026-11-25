; Lunaris Server installer for Windows: installs lunaris-server.exe and runs it
; as the LunarisServer Windows service (the same service, data folder and
; firewall rule as `lunaris-server install`, see server/src/service.rs).
; Everything else is managed from the web console.
;
; Built in CI from the repository root:
;   makensis /DVERSION=<x.y.z> scripts\packaging\server-installer.nsi
; expecting .\lunaris-server\lunaris-server.exe (the "Package Server" step).
;
; Data: %ProgramData%\Lunaris\Server (database, server.env, server.log, keys
; and, until the first admin is made, setup_code.txt), SYSTEM and
; Administrators only. The folder above it is the agent's service's
; (agent_config.json, agent-service.log); older servers kept their files there
; too, and `lunaris-server prepare-data-dir` moves those (only those) into the
; server's own folder. Uninstall removes the server's folder only when asked.

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif

!define APP      "Lunaris Server"
!define EXE      "lunaris-server.exe"
!define SERVICE  "LunarisServer"
; Same name as `lunaris-server install` uses, so either replaces the other's.
!define FIREWALL "Lunaris Server"
!define UNINST   "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}"

Name "${APP}"
SetCompressor /SOLID lzma
OutFile "..\..\lunaris-server-installer.exe"
InstallDir "$PROGRAMFILES64\${APP}"
InstallDirRegKey HKLM "Software\${APP}" "InstallDir"
RequestExecutionLevel admin
Unicode true

Var DataDir
Var Port
Var Fresh            ; 1: no service before this install
Var WasRunning       ; 1: the service ran before this install
Var ShowCode         ; 1: setup_code.txt exists (new database, no account yet)
Var GatewayNote      ; what the finish page says about the server gateway

!define MUI_ABORTWARNING
!define MUI_ICON   "..\..\agent\icons\icon.ico"
!define MUI_UNICON "..\..\agent\icons\icon.ico"

!define MUI_FINISHPAGE_TITLE "Lunaris Server is running"
!define MUI_FINISHPAGE_TEXT "The server runs as the $\"${APP}$\" Windows service and starts with Windows. Manage it from the web console: http://localhost:$Port (or http://<this computer>:$Port from another one).$\r$\n$\r$\nOn a new install, the web console asks for the one-time setup code in $DataDir\setup_code.txt (administrators only) and then for the first administrator's name and password.$\r$\n$\r$\nSettings: $DataDir\server.env (restart the service after editing).$GatewayNote"
!define MUI_FINISHPAGE_LINK "Open the web console"
!define MUI_FINISHPAGE_LINK_LOCATION "http://localhost:$Port/"
!define MUI_FINISHPAGE_SHOWREADME ""
!define MUI_FINISHPAGE_SHOWREADME_TEXT "Show the setup code"
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION ShowSetupCode

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\..\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
; (the show function applies to the next page only: the finish page)
!define MUI_PAGE_CUSTOMFUNCTION_SHOW FinishShow
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; %ProgramData%\Lunaris\Server, as data_dir() in server/src/service.rs.
!macro SetDataDir
  SetShellVarContext all
  StrCpy $DataDir "$APPDATA\Lunaris\Server"
  SetShellVarContext current
!macroend

; The install folder: the service runs its program as LocalSystem, so only
; SYSTEM and Administrators may change it (the Section restricts its access).
; So that this never changes the access of someone else's files, it must be
; empty or an earlier install of this server, and not inside ProgramData
; (the agent's and the server's data folders, which have access of their own).
; Result in $InstDirOk.
Var InstDirOk
Function CheckInstDir
  Push $0
  Push $1
  Push $2
  StrCpy $InstDirOk 1
  SetShellVarContext all
  StrCpy $2 "$APPDATA"
  SetShellVarContext current
  StrLen $1 $2
  StrCpy $0 "$INSTDIR" $1
  StrCmp $0 $2 0 cid_notdata
    StrCpy $InstDirOk 0
    Goto cid_done
  cid_notdata:
  IfFileExists "$INSTDIR\*.*" 0 cid_done
  IfFileExists "$INSTDIR\${EXE}" cid_done
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
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

Function .onVerifyInstDir
  Call CheckInstDir
  StrCmp $InstDirOk 1 +2
    Abort
FunctionEnd

; SERVER_PORT from server.env into $Port (8090 when unset).
Function ReadPort
  Push $0
  Push $1
  Push $2
  StrCpy $Port "8090"
  ClearErrors
  FileOpen $0 "$DataDir\server.env" r
  IfErrors rp_done
  rp_loop:
    ClearErrors
    FileRead $0 $1
    IfErrors rp_close
    StrCpy $2 $1 12
    StrCmp $2 "SERVER_PORT=" 0 rp_loop
    StrCpy $1 $1 "" 12
  rp_trim:
    StrCpy $2 $1 1 -1
    StrCmp $2 "$\n" rp_cut
    StrCmp $2 "$\r" rp_cut
    StrCmp $2 " " rp_cut
    Goto rp_trimmed
  rp_cut:
    StrCpy $1 $1 -1
    Goto rp_trim
  rp_trimmed:
    StrCmp $1 "" rp_loop
    StrCpy $Port $1
    Goto rp_loop
  rp_close:
  FileClose $0
  rp_done:
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

Function FinishShow
  ${If} $ShowCode != 1
    ShowWindow $mui.FinishPage.ShowReadme ${SW_HIDE}
  ${EndIf}
FunctionEnd

; Notepad runs elevated from here, so it can open the administrators-only file.
Function ShowSetupCode
  ${If} $ShowCode == 1
    Exec '"$WINDIR\notepad.exe" "$DataDir\setup_code.txt"'
  ${EndIf}
FunctionEnd

Section "Install"
  !insertmacro SetDataDir

  ; An existing service (an earlier version, or `lunaris-server install`):
  ; stop it so the program can be replaced.
  StrCpy $Fresh 1
  StrCpy $WasRunning 0
  nsExec::Exec '"$SYSDIR\sc.exe" query ${SERVICE}'
  Pop $0
  ${If} $0 == 0
    StrCpy $Fresh 0
    nsExec::Exec '"$SYSDIR\net.exe" stop ${SERVICE}'
    Pop $0
    ${If} $0 == 0
      StrCpy $WasRunning 1
    ${EndIf}
  ${EndIf}

  ; The service runs this folder's program as LocalSystem: only SYSTEM and
  ; Administrators may change the folder (users may read and run it), also
  ; when another folder was chosen (C:\Lunaris would otherwise let every
  ; signed-in user replace the program). Restricted before the files are
  ; copied, and again on every upgrade; SIDs, not names (localized Windows).
  ; The data folder in ProgramData is not touched.
  Call CheckInstDir
  StrCmp $InstDirOk 1 +3
    MessageBox MB_OK|MB_ICONSTOP "$INSTDIR is not empty (or is inside ProgramData). Choose an empty folder or the folder of an earlier Lunaris Server install." /SD IDOK
    Abort
  CreateDirectory "$INSTDIR"
  nsExec::ExecToLog 'icacls "$INSTDIR" /inheritance:r /grant:r *S-1-5-18:(OI)(CI)F *S-1-5-32-544:(OI)(CI)F *S-1-5-32-545:(OI)(CI)RX'
  Pop $0
  StrCmp $0 0 +3
    MessageBox MB_OK|MB_ICONSTOP "Could not restrict access to $INSTDIR (icacls: $0)." /SD IDOK
    Abort

  SetOutPath "$INSTDIR"
  File "..\..\lunaris-server\${EXE}"
  File "..\..\LICENSE"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  ; Files of an earlier install with access of their own (not inherited):
  ; back to the folder's.
  nsExec::ExecToLog 'icacls "$INSTDIR\${EXE}" /reset'
  Pop $0
  nsExec::ExecToLog 'icacls "$INSTDIR\LICENSE" /reset'
  Pop $0
  nsExec::ExecToLog 'icacls "$INSTDIR\uninstall.exe" /reset'
  Pop $0
  WriteRegStr HKLM "Software\${APP}" "InstallDir" "$INSTDIR"

  ; The data folder holds the server's secrets. The program (the one just
  ; installed, with the service stopped) refuses a folder that is or holds a
  ; link, makes it SYSTEM + Administrators only with nothing inherited from
  ; ProgramData (where every user may create folders), and moves an older
  ; server's files out of %ProgramData%\Lunaris (service.rs prepare_data_dir).
  nsExec::ExecToLog '"$INSTDIR\${EXE}" prepare-data-dir'
  Pop $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Could not prepare the data folder $DataDir (is it a link?). Remove it, or fix it, and run the installer again." /SD IDOK
    Abort
  ${EndIf}

  ; Settings, written once (the same defaults as `lunaris-server install`).
  ${IfNot} ${FileExists} "$DataDir\server.env"
    FileOpen $0 "$DataDir\server.env" w
    FileWrite $0 "# Lunaris server settings (KEY=value). Restart the service after editing.$\r$\n"
    FileWrite $0 "SERVER_PORT=8090$\r$\n"
    FileWrite $0 "# Token agents use to register (generated on first start when unset):$\r$\n"
    FileWrite $0 "# LUNARIS_TOKEN=$\r$\n"
    FileWrite $0 "# Log level:$\r$\n"
    FileWrite $0 "# RUST_LOG=info$\r$\n"
    FileClose $0
  ${EndIf}
  Call ReadPort

  ; The service runs `lunaris-server.exe service` as LocalSystem (what
  ; `lunaris-server install` registers), starts with Windows and restarts
  ; after a crash.
  ${If} $Fresh == 1
    nsExec::Exec '"$SYSDIR\sc.exe" create ${SERVICE} binPath= "\"$INSTDIR\${EXE}\" service" start= auto DisplayName= "${APP}"'
  ${Else}
    nsExec::Exec '"$SYSDIR\sc.exe" config ${SERVICE} binPath= "\"$INSTDIR\${EXE}\" service"'
  ${EndIf}
  Pop $0
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Could not register the ${SERVICE} service (sc: $0)." /SD IDOK
    Abort
  ${EndIf}
  nsExec::Exec '"$SYSDIR\sc.exe" description ${SERVICE} "Lunaris remote desktop server (web console, accounts, signaling)"'
  Pop $0
  nsExec::Exec '"$SYSDIR\sc.exe" failure ${SERVICE} reset= 86400 actions= restart/5000/restart/5000/restart/30000'
  Pop $0

  ; Agents, browsers and the TURN relay (UDP) reach the program.
  nsExec::Exec '"$SYSDIR\netsh.exe" advfirewall firewall delete rule name="${FIREWALL}"'
  Pop $0
  nsExec::Exec '"$SYSDIR\netsh.exe" advfirewall firewall add rule name="${FIREWALL}" dir=in action=allow program="$INSTDIR\${EXE}" enable=yes'
  Pop $0

  WriteRegStr HKLM "${UNINST}" "DisplayName" "${APP}"
  WriteRegStr HKLM "${UNINST}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST}" "Publisher" "Lunaris"
  WriteRegStr HKLM "${UNINST}" "DisplayIcon" '"$INSTDIR\${EXE}"'
  WriteRegStr HKLM "${UNINST}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "${UNINST}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegStr HKLM "${UNINST}" "URLInfoAbout" "https://github.com/collyn/lunaris"
  WriteRegDWORD HKLM "${UNINST}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST}" "NoRepair" 1

  ; A new install starts the service; an upgrade starts it again only when it
  ; was running (an administrator may have stopped it on purpose).
  ${If} $Fresh == 1
  ${OrIf} $WasRunning == 1
    nsExec::Exec '"$SYSDIR\net.exe" start ${SERVICE}'
    Pop $0
    ${If} $0 != 0
      MessageBox MB_OK|MB_ICONEXCLAMATION "The ${SERVICE} service did not start (net: $0). See $DataDir\server.log." /SD IDOK
    ${EndIf}
  ${EndIf}

  ; The server gateway (server/src/gateway.rs) runs Lunaris Agent's
  ; lunaris-agent.exe, found where its installer put it, to reach RustDesk,
  ; Sunshine, RDP and VNC computers through this server. Not bundled here.
  StrCpy $GatewayNote ""
  ReadRegStr $1 HKLM "Software\Lunaris Agent" "InstallDir"
  ${If} $1 == ""
    StrCpy $1 "$PROGRAMFILES64\Lunaris Agent"
  ${EndIf}
  ${IfNot} ${FileExists} "$1\lunaris-agent.exe"
    StrCpy $GatewayNote "$\r$\n$\r$\nTo add RustDesk, Sunshine, RDP and VNC computers through this server itself (its gateway), also install Lunaris Agent on this computer; the server finds it on its own."
  ${EndIf}

  ; A server without accounts writes the setup code when it starts.
  StrCpy $ShowCode 0
  ${If} $Fresh == 1
  ${OrIf} ${FileExists} "$DataDir\setup_code.txt"
    StrCpy $1 0
    ${Do}
      ${If} ${FileExists} "$DataDir\setup_code.txt"
        StrCpy $ShowCode 1
        ${Break}
      ${EndIf}
      Sleep 250
      IntOp $1 $1 + 1
    ${LoopUntil} $1 >= 60
  ${EndIf}
SectionEnd

Section "Uninstall"
  !insertmacro SetDataDir
  nsExec::Exec '"$SYSDIR\net.exe" stop ${SERVICE}'
  Pop $0
  nsExec::Exec '"$SYSDIR\sc.exe" delete ${SERVICE}'
  Pop $0
  nsExec::Exec '"$SYSDIR\netsh.exe" advfirewall firewall delete rule name="${FIREWALL}"'
  Pop $0

  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "${UNINST}"
  DeleteRegKey HKLM "Software\${APP}"

  ; Kept unless asked (a silent uninstall keeps them): accounts, hosts and
  ; settings come back with a reinstall. The folder is the server's own;
  ; the agent's files are in the folder above it and stay.
  MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "Also delete the server's data (accounts, computers, settings, keys) in $DataDir?" /SD IDNO IDNO un_keep
    ${GetFileAttributes} "$DataDir" "REPARSE_POINT" $0
    ${If} $0 == 1
      MessageBox MB_OK|MB_ICONEXCLAMATION "$DataDir is a link: not deleted." /SD IDOK
    ${Else}
      RMDir /r "$DataDir"
    ${EndIf}
  un_keep:
SectionEnd
