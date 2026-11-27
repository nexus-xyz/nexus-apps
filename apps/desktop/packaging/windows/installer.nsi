; Nexus's Windows installer, built by packaging/package.ps1 with NSIS (makensis). It installs for the current user
; only, so it asks for no administrator rights: the app in %LOCALAPPDATA%\Programs\Nexus, a Start menu shortcut, and an
; uninstaller listed in Settings > Apps. Solid LZMA compression keeps the download small.
;
; package.ps1 defines VERSION, STAGE (the folder with the exe and CEF's files), ICON and OUTFILE.
;
; The app's updater (src/updates/install.rs) runs it as `setup.exe /S /UPDATE` and quits. /UPDATE waits for that copy
; to close, then starts the new version when the install is done.
;
; Builds before the rename were "Nexus GPUI", in %LOCALAPPDATA%\Programs\Nexus GPUI. Install removes that folder and its
; shortcut. The uninstall key keeps its old name, NexusGPUI, so Settings > Apps lists one entry, not two.
;
; An older Nexus install (0.10.x and before, per user) used this same folder, as "Nexus". Install takes its place: it stops nexus-desktop.exe, drops its Apps entry and its own registry key, and
; points its desktop shortcut, login item (Run\Nexus, shared by name with this app) and nexus:// handler at this exe.
; Its data folder and sign-in stay.

Unicode true
!include "FileFunc.nsh"
!include "LogicLib.nsh"
SetCompressor /SOLID lzma
SetCompressorDictSize 64

!define APP "Nexus"
!define LEGACY_APP "Nexus GPUI"
!define EXE "nexus-desktop-gpui.exe"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\NexusGPUI"
!define OLD_EXE "nexus-desktop.exe"
!define OLD_UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nexus"
!define OLD_KEY "Software\Nexus\Nexus"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
; The login item's name and argument (nexus-desktop-core's LOGIN_ITEM and AUTOSTART_ARG). auto-launch 0.6 writes the
; path unquoted (AUTOSTART_APP); the installer quotes it, which survives a space in the user's folder.
!define LOGIN_ITEM "Nexus"
!define AUTOSTART '"$INSTDIR\${EXE}" --autostart'
!define AUTOSTART_APP '$INSTDIR\${EXE} --autostart'
!define SCHEME_KEY "Software\Classes\nexus"
!define SCHEME_COMMAND '"$INSTDIR\${EXE}" "%1"'

Var Update

Name "${APP}"
OutFile "${OUTFILE}"
Icon "${ICON}"
UninstallIcon "${ICON}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\${APP}"
ShowInstDetails nevershow
ShowUninstDetails nevershow

; No folder choice: install and uninstall remove the whole folder, which must only ever be the app's own.
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "Install"
  StrCpy $Update 0
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/UPDATE" $R1
  ${IfNot} ${Errors}
    StrCpy $Update 1
    ; The app quits on its own after it starts this installer: give it time before the kill below.
    Sleep 3000
  ${EndIf}
  ; A running copy holds its files open, an older Nexus install's too.
  nsExec::Exec 'taskkill /IM "${EXE}" /F'
  nsExec::Exec 'taskkill /IM "${OLD_EXE}" /F'
  ; The install from before the rename, so two copies never sit side by side.
  RMDir /r "$LOCALAPPDATA\Programs\${LEGACY_APP}"
  Delete "$SMPROGRAMS\${LEGACY_APP}.lnk"
  ; An older Nexus install (0.10.x and before) in this folder: its Apps entry would run this app's uninstaller, and
  ; its desktop shortcut would open the exe deleted below.
  ${If} ${FileExists} "$INSTDIR\${OLD_EXE}"
    DeleteRegKey HKCU "${OLD_UNINSTALL_KEY}"
    DeleteRegKey HKCU "${OLD_KEY}"
    ${If} ${FileExists} "$DESKTOP\${APP}.lnk"
      CreateShortcut "$DESKTOP\${APP}.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\icon.ico"
    ${EndIf}
  ${EndIf}
  ; Files of an older version must not linger beside the new ones.
  RMDir /r "$INSTDIR"
  SetOutPath "$INSTDIR"
  File /r "${STAGE}\*.*"
  File "/oname=icon.ico" "${ICON}"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortcut "$SMPROGRAMS\${APP}.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\icon.ico"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "Nexus"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\icon.ico"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  ; A login item that is on, maybe still pointing at an older install's exe, now starts this one. Off stays off.
  ReadRegStr $R2 HKCU "${RUN_KEY}" "${LOGIN_ITEM}"
  ${If} $R2 != ""
    WriteRegStr HKCU "${RUN_KEY}" "${LOGIN_ITEM}" '${AUTOSTART}'
  ${EndIf}
  ; nexus:// links (links.rs reads them from the arguments).
  WriteRegStr HKCU "${SCHEME_KEY}" "" "URL:Nexus protocol"
  WriteRegStr HKCU "${SCHEME_KEY}" "URL Protocol" ""
  WriteRegStr HKCU "${SCHEME_KEY}\DefaultIcon" "" '"$INSTDIR\${EXE}",0'
  WriteRegStr HKCU "${SCHEME_KEY}\shell\open\command" "" '${SCHEME_COMMAND}'
  ${If} $Update == 1
    Exec '"$INSTDIR\${EXE}"'
  ${EndIf}
SectionEnd

Section "Uninstall"
  nsExec::Exec 'taskkill /IM "${EXE}" /F'
  Delete "$SMPROGRAMS\${APP}.lnk"
  ; The login item and the nexus:// handler, only while they still start this exe.
  ReadRegStr $R2 HKCU "${RUN_KEY}" "${LOGIN_ITEM}"
  ${If} $R2 == '${AUTOSTART}'
  ${OrIf} $R2 == '${AUTOSTART_APP}'
    DeleteRegValue HKCU "${RUN_KEY}" "${LOGIN_ITEM}"
  ${EndIf}
  ReadRegStr $R2 HKCU "${SCHEME_KEY}\shell\open\command" ""
  ${If} $R2 == '${SCHEME_COMMAND}'
    DeleteRegKey HKCU "${SCHEME_KEY}"
  ${EndIf}
  RMDir /r "$INSTDIR"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
SectionEnd
