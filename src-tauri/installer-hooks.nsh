; Backtrack's additions to Tauri's NSIS installer (see tauri.installer.conf.json).
;
; Never touch Videos\Backtrack here: that's where people's recordings and exports live.

; Close Backtrack and the bundled ffmpeg it runs from this folder, so their files can be replaced
; or removed. Only processes started from $INSTDIR are stopped (not a portable copy, not other ffmpegs).
!macro BACKTRACK_STOP_PROCESSES
  nsExec::Exec `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "Get-Process backtrack,ffmpeg,ffprobe -ErrorAction SilentlyContinue | Where-Object { $$_.Path -like '$INSTDIR\*' } | Stop-Process -Force; Start-Sleep -Milliseconds 300"`
  Pop $0
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro BACKTRACK_STOP_PROCESSES
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro BACKTRACK_STOP_PROCESSES
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    ; "Start with Windows" adds this value. Don't leave it pointing at a deleted exe. (After an upgrade,
    ; the new version writes it back on its first launch.)
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Backtrack"

    ; "Delete the application data" was ticked: remove Backtrack's settings and cache too
    ; (Tauri's own clean-up only knows the bundle-identifier folders).
    ${If} $DeleteAppDataCheckboxState = 1
      RMDir /r "$APPDATA\Backtrack"
      RMDir /r "$LOCALAPPDATA\Backtrack\audio"
      RMDir /r "$LOCALAPPDATA\Backtrack\thumbs"
      RMDir /r "$LOCALAPPDATA\Backtrack\passlog"
      RMDir /r "$LOCALAPPDATA\Backtrack\e2e-exports"
      Delete "$LOCALAPPDATA\Backtrack\index.json"
      RMDir "$LOCALAPPDATA\Backtrack"
    ${EndIf}
  ${EndIf}
!macroend
