!include "MUI2.nsh"

!define EXE "client-desktop.exe"

Name "Lunaris Client"
; One LZMA stream over all files: about a third smaller than the default.
SetCompressor /SOLID lzma
OutFile "lunaris-client-installer.exe"
InstallDir "$PROGRAMFILES64\Lunaris Client"
; (no InstallDirRegKey: nothing writes it, and a user-writable key must not steer an elevated install)

RequestExecutionLevel admin

!define MUI_ABORTWARNING
!define MUI_ICON "client-desktop\icons\icon.ico"
!define MUI_UNICON "client-desktop\icons\icon.ico"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_WELCOME
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH

!insertmacro MUI_LANGUAGE "English"


; The agent's Windows service (LunarisAgentService, `--install-service`, see
; agent/src/win_service.rs) runs the program from its install folder, which
; keeps the files in use. An install (also the app's silent update, `/S`)
; stops it before copying and starts it again afterwards when it was running.
Var ServiceWasRunning

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

Section "Install"
  Call CheckInstDir
  StrCmp $InstDirOk 1 +3
    MessageBox MB_OK|MB_ICONSTOP "The install folder $INSTDIR is not empty and is not a previous install. Choose an empty folder." /SD IDOK
    Abort
  ; Stop the agent service (if installed and running) so its files can be replaced.
  StrCpy $ServiceWasRunning 0
  nsExec::Exec 'net stop LunarisAgentService'
  Pop $0
  StrCmp $0 0 0 svc_not_running
    StrCpy $ServiceWasRunning 1
  svc_not_running:
  SetOutPath "$INSTDIR"
  File /r "lunaris-client-desktop\*"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  ; Record what was installed so the uninstaller removes exactly that.
  FileOpen $LogFile "$INSTDIR\install.log" w
  Push ""
  Call LogDir
  FileClose $LogFile

  # Create Start Menu shortcuts
  CreateDirectory "$SMPROGRAMS\Lunaris Client"
  CreateShortcut "$SMPROGRAMS\Lunaris Client\Lunaris Client.lnk" "$INSTDIR\client-desktop.exe"
  CreateShortcut "$SMPROGRAMS\Lunaris Client\Uninstall Lunaris Client.lnk" "$INSTDIR\uninstall.exe"

  # Create Desktop shortcut
  CreateShortcut "$DESKTOP\Lunaris Client.lnk" "$INSTDIR\client-desktop.exe"

  # Write registry keys for Add/Remove Programs
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunaris Client" "DisplayName" "Lunaris Client"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunaris Client" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunaris Client" "DisplayIcon" '"$INSTDIR\client-desktop.exe"'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunaris Client" "Publisher" "Lunaris"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunaris Client" "DisplayVersion" "0.6.6"
  ; Start the service again with the new files.
  StrCmp $ServiceWasRunning 1 0 svc_done
    nsExec::Exec 'net start LunarisAgentService'
    Pop $0
  svc_done:
SectionEnd

Section "Uninstall"
  Delete "$SMPROGRAMS\Lunaris Client\Lunaris Client.lnk"
  Delete "$SMPROGRAMS\Lunaris Client\Uninstall Lunaris Client.lnk"
  RMDir "$SMPROGRAMS\Lunaris Client"
  Delete "$DESKTOP\Lunaris Client.lnk"

  # Starting with Windows: the scheduled task (and the Run entry of older versions)
  nsExec::Exec 'schtasks /Delete /TN "LunarisClient" /F'
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "LunarisClient"

  # Delete registry keys
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunaris Client"

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
SectionEnd
