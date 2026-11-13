; Lunaris Agent installer for Windows.
; Built in CI from the repository root:
;   makensis /DVERSION=<x.y.z> scripts\packaging\agent-installer.nsi
; expecting the deployed agent (windeployqt + DLLs) in .\lunaris-agent\.

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
!define MUI_FINISHPAGE_TEXT "Lunaris Agent is installed.$\r$\n$\r$\nStart it from the Start menu, enter your server address and token, then click Start. Enable $\"Start at login$\" to keep this computer reachable."

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\..\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Install"
  ; Replace a running copy on upgrade.
  nsExec::Exec 'taskkill /IM "${EXE}" /F'

  SetOutPath "$INSTDIR"
  File /r "..\..\lunaris-agent\*"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKLM "Software\${APP}" "InstallDir" "$INSTDIR"

  CreateDirectory "$SMPROGRAMS\${APP}"
  CreateShortcut "$SMPROGRAMS\${APP}\${APP}.lnk" "$INSTDIR\${EXE}"
  CreateShortcut "$SMPROGRAMS\${APP}\Uninstall ${APP}.lnk" "$INSTDIR\uninstall.exe"
  CreateShortcut "$DESKTOP\${APP}.lnk" "$INSTDIR\${EXE}"

  ; Let peers reach the agent directly (WebRTC P2P / WebTransport).
  nsExec::Exec 'netsh advfirewall firewall delete rule name="${APP}"'
  nsExec::Exec 'netsh advfirewall firewall add rule name="${APP}" dir=in action=allow program="$INSTDIR\${EXE}" enable=yes'

  WriteRegStr HKLM "${UNINST}" "DisplayName" "${APP}"
  WriteRegStr HKLM "${UNINST}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST}" "Publisher" "Lunaris"
  WriteRegStr HKLM "${UNINST}" "DisplayIcon" '"$INSTDIR\${EXE}"'
  WriteRegStr HKLM "${UNINST}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "${UNINST}" "URLInfoAbout" "https://github.com/collyn/lunaris"
  WriteRegDWORD HKLM "${UNINST}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  nsExec::Exec 'taskkill /IM "${EXE}" /F'
  ; "Start at login" entry written by the agent (current user).
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "LunarisAgent"
  nsExec::Exec 'netsh advfirewall firewall delete rule name="${APP}"'

  Delete "$SMPROGRAMS\${APP}\${APP}.lnk"
  Delete "$SMPROGRAMS\${APP}\Uninstall ${APP}.lnk"
  RMDir "$SMPROGRAMS\${APP}"
  Delete "$DESKTOP\${APP}.lnk"

  DeleteRegKey HKLM "${UNINST}"
  DeleteRegKey HKLM "Software\${APP}"
  RMDir /r "$INSTDIR"
  ; Settings in %APPDATA%\lunaris-agent are kept.
SectionEnd
