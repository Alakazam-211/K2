; Kill the detached daemon before NSIS writes %LOCALAPPDATA%\K2\*.
; k2-daemon is spawned with CREATE_NEW_PROCESS_GROUP, so closing or
; uninstalling k2.exe leaves it running and locks k2-daemon.exe
; ("Error opening file for writing").
;
; /T also takes the daemon's frpc child. Do not taskkill frpc.exe by
; name — that would hit a user's own tunnel client.

!macro NSIS_HOOK_PREINSTALL
  nsExec::ExecToLog 'taskkill /F /IM k2.exe /T'
  nsExec::ExecToLog 'taskkill /F /IM k2-daemon.exe /T'
  Sleep 800
  Delete /REBOOTOK "$INSTDIR\k2.exe"
  Delete /REBOOTOK "$INSTDIR\k2-daemon.exe"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog 'taskkill /F /IM k2.exe /T'
  nsExec::ExecToLog 'taskkill /F /IM k2-daemon.exe /T'
  Sleep 800
!macroend
