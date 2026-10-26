Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "WinVer.nsh"
!include "x64.nsh"
!ifndef BUNDLE
  !error "Pass -DBUNDLE=<complete Jarvis folder>"
!endif
!ifndef OUTPUT
  !define OUTPUT "Jarvis-0.2.0-Windows-x64-Setup.exe"
!endif
Name "Jarvis 0.2.0"
OutFile "${OUTPUT}"
InstallDir "$LOCALAPPDATA\Programs\Jarvis"
RequestExecutionLevel user
SetCompressor /SOLID lzma
SetCompressorDictSize 32
VIProductVersion "0.2.0.0"
VIAddVersionKey "ProductName" "Jarvis"
VIAddVersionKey "FileDescription" "Jarvis offline desktop voice controls"
VIAddVersionKey "FileVersion" "0.2.0"
VIAddVersionKey "LegalCopyright" "Jarvis contributors; MIT"
!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "../../LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "Jarvis requires 64-bit Windows 10 or Windows 11."
    Abort
  ${EndIf}
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_ICONSTOP "Jarvis requires Windows 10 or Windows 11."
    Abort
  ${EndIf}
  SetShellVarContext current
  FindWindow $0 "TkTopLevel" "Jarvis · Settings"
  ${If} $0 != 0
    MessageBox MB_ICONINFORMATION "Close the Jarvis Settings window, then run this installer again. The background service will be stopped automatically."
    Abort
  ${EndIf}
FunctionEnd

Section "Jarvis" Main
  ; Ask an existing instance to stop its owned workers before replacing files.
  IfFileExists "$INSTDIR\JarvisConsole.exe" 0 ready
  ExecWait '"$INSTDIR\JarvisConsole.exe" shutdown' $0
  ${If} $0 != 0
    MessageBox MB_ICONSTOP "Jarvis could not finish shutting down. Stop it from the tray, then try again."
    Abort
  ${EndIf}
  ready:
  SetOutPath "$INSTDIR"
  File /r "${BUNDLE}/*"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\Jarvis"
  CreateShortcut "$SMPROGRAMS\Jarvis\Jarvis.lnk" "$INSTDIR\Jarvis.exe"
  CreateShortcut "$SMPROGRAMS\Jarvis\Uninstall.lnk" "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Jarvis" "DisplayName" "Jarvis"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Jarvis" "DisplayVersion" "0.2.0"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Jarvis" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Jarvis" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Jarvis" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Jarvis" "NoRepair" 1
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  FindWindow $0 "TkTopLevel" "Jarvis · Settings"
  ${If} $0 != 0
    MessageBox MB_ICONINFORMATION "Close Jarvis Settings before uninstalling."
    Abort
  ${EndIf}
  IfFileExists "$INSTDIR\BUILD-MANIFEST.json" +3 0
    MessageBox MB_ICONSTOP "The Jarvis installation manifest is missing. No files were removed."
    Abort
  ExecWait '"$INSTDIR\JarvisConsole.exe" shutdown' $0
  ${If} $0 != 0
    MessageBox MB_ICONSTOP "Jarvis could not finish shutting down. Stop it from the tray, then try again."
    Abort
  ${EndIf}
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Jarvis"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Jarvis"
  RMDir /r "$SMPROGRAMS\Jarvis"
  ; Program files only. Never delete the user's recordings or account settings.
  RMDir /r "$INSTDIR\_internal"
  RMDir /r "$INSTDIR\models"
  RMDir /r "$INSTDIR\tools"
  RMDir /r "$INSTDIR\licenses"
  Delete "$INSTDIR\Jarvis.exe"
  Delete "$INSTDIR\JarvisConsole.exe"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\THIRD-PARTY-NOTICES.txt"
  Delete "$INSTDIR\BUILD-MANIFEST.json"
  Delete "$INSTDIR\FFmpeg-Corresponding-Source.tar.xz"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
SectionEnd
