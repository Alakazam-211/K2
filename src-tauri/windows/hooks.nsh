; Kill the detached daemon before NSIS writes %LOCALAPPDATA%\K2\*.
; k2-daemon is spawned with CREATE_NEW_PROCESS_GROUP, so closing or
; uninstalling k2.exe leaves it running and locks k2-daemon.exe
; ("Error opening file for writing").
;
; /T on the daemon is not enough: frpc is often already orphaned
; (daemon died, tunnel left running) and NSIS then cannot write
; $INSTDIR\frpc.exe. Kill the bundled name too. A stray user frpc
; of the same image name is rare; install would fail either way.

!macro NSIS_HOOK_PREINSTALL
  nsExec::ExecToLog 'taskkill /F /IM k2.exe /T'
  nsExec::ExecToLog 'taskkill /F /IM k2-daemon.exe /T'
  nsExec::ExecToLog 'taskkill /F /IM frpc.exe /T'
  Sleep 800
  Delete /REBOOTOK "$INSTDIR\k2.exe"
  Delete /REBOOTOK "$INSTDIR\k2-daemon.exe"
  Delete /REBOOTOK "$INSTDIR\frpc.exe"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog 'taskkill /F /IM k2.exe /T'
  nsExec::ExecToLog 'taskkill /F /IM k2-daemon.exe /T'
  nsExec::ExecToLog 'taskkill /F /IM frpc.exe /T'
  Sleep 800
!macroend
