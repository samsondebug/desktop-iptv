; NSIS hooks for the SKTV installer (tauri.conf.json → bundle.windows.nsis.installerHooks).
;
; SKTV shipped as "desktop-iptv" up to 0.1.0. Tauri's NSIS template keys the uninstall entry and
; the install folder on the product name, so the stock "previous version found" logic cannot see
; a desktop-iptv install and would leave it behind — two copies, and the old one offering this
; update forever. Before installing, remove a leftover desktop-iptv (per-machine and per-user).
;
; App data is untouched: the bundle identifier (dev.desktopiptv.app) did not change, and a
; silent NSIS uninstall never deletes %APPDATA%\<identifier> (that needs the checkbox in the
; interactive uninstaller). The catalog, settings, licence and log all carry over.

!macro RemoveLegacyDesktopIptv ROOT
  ClearErrors
  ReadRegStr $R0 ${ROOT} "Software\Microsoft\Windows\CurrentVersion\Uninstall\desktop-iptv" "UninstallString"
  ${If} $R0 != ""
    ; The old template stores the unquoted install dir under Software\<publisher>\<product>.
    ReadRegStr $R1 ${ROOT} "Software\desktopiptv\desktop-iptv" ""
    ${If} $R1 == ""
      ; Fall back to InstallLocation, which is stored quoted.
      ReadRegStr $R1 ${ROOT} "Software\Microsoft\Windows\CurrentVersion\Uninstall\desktop-iptv" "InstallLocation"
      StrCpy $R2 $R1 1
      ${If} $R2 == '"'
        StrLen $R3 $R1
        IntOp $R3 $R3 - 2
        StrCpy $R1 $R1 $R3 1
      ${EndIf}
    ${EndIf}
    DetailPrint "Removing the previous desktop-iptv installation from $R1"
    ; /S = silent (kills a running desktop-iptv.exe by itself); _?= runs the uninstaller in
    ; place so ExecWait really waits, which also means it cannot delete itself — done below.
    ExecWait '$R0 /S _?=$R1' $R4
    ${If} $R1 != ""
      Delete "$R1\uninstall.exe"
      RMDir "$R1"
    ${EndIf}
    DeleteRegKey ${ROOT} "Software\Microsoft\Windows\CurrentVersion\Uninstall\desktop-iptv"
    DeleteRegKey ${ROOT} "Software\desktopiptv\desktop-iptv"
    DeleteRegKey /ifempty ${ROOT} "Software\desktopiptv"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro RemoveLegacyDesktopIptv HKLM
  !insertmacro RemoveLegacyDesktopIptv HKCU
!macroend
