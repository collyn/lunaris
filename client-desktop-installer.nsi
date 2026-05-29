!include "MUI2.nsh"

Name "Lunaris Client"
OutFile "lunaris-client-installer.exe"
InstallDir "$PROGRAMFILES64\Lunaris Client"
InstallDirRegKey HKCU "Software\Lunaris Client" ""

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

Section "Install"
  SetOutPath "$INSTDIR"
  File /r "lunaris-client-desktop\*"

  WriteUninstaller "$INSTDIR\uninstall.exe"

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
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunaris Client" "DisplayVersion" "0.1.7"
SectionEnd

Section "Uninstall"
  Delete "$SMPROGRAMS\Lunaris Client\Lunaris Client.lnk"
  Delete "$SMPROGRAMS\Lunaris Client\Uninstall Lunaris Client.lnk"
  RMDir "$SMPROGRAMS\Lunaris Client"
  Delete "$DESKTOP\Lunaris Client.lnk"
  
  # Delete registry keys
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lunaris Client"

  RMDir /r "$INSTDIR"
SectionEnd
