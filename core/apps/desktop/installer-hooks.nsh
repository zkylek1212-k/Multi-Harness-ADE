; Moving from the Electron build: it installed elsewhere, under the GUID electron-builder derives from the app id, and would otherwise
; stay in Add/Remove Programs with its own shortcuts. Its uninstaller leaves userData alone (deleteAppDataOnUninstall: false), and
; userData is what this version keeps using.
!define OLD_ELECTRON_GUID "32f50c10-6c19-544e-8a7c-56f78cfa29ce"
!define OLD_ELECTRON_UNINSTALLER "Uninstall Agent Workbench.exe"

!macro NSIS_HOOK_PREINSTALL
  ReadRegStr $R0 HKCU "Software\${OLD_ELECTRON_GUID}" "InstallLocation"
  StrCmp $R0 "" old_electron_done
  IfFileExists "$R0\${OLD_ELECTRON_UNINSTALLER}" 0 old_electron_done
    DetailPrint "Removing the previous version (your settings stay)"
    ; _?= makes it run in place and really wait. Without it the uninstaller copies itself away and returns at once, and could then
    ; delete the Start menu shortcut this installer is about to create. The last word on that switch is the folder.
    ExecWait '"$R0\${OLD_ELECTRON_UNINSTALLER}" /currentuser /S _?=$R0' $R1
    ; A failure is not fatal: both versions work side by side, and the old entry stays for the user to remove
    DetailPrint "Previous version's uninstaller returned $R1"
    Delete "$R0\${OLD_ELECTRON_UNINSTALLER}"
    RMDir "$R0"
  old_electron_done:
!macroend
