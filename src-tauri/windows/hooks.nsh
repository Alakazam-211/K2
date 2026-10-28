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

; K2's command-line tools (the `k2` CLI agents call) are a bash script:
; without Git for Windows they can't run. Say so once, plainly, at the end
; of the install, and offer the download page. Silent installs skip the
; question (/SD IDNO).
!macro NSIS_HOOK_POSTINSTALL
  StrCpy $0 ""
  ReadRegStr $0 HKLM "SOFTWARE\GitForWindows" "InstallPath"
  StrCmp $0 "" 0 k2_git_check
  ReadRegStr $0 HKCU "SOFTWARE\GitForWindows" "InstallPath"
  StrCmp $0 "" 0 k2_git_check
  StrCpy $0 "$LOCALAPPDATA\Programs\Git"
  k2_git_check:
  IfFileExists "$0\bin\bash.exe" k2_git_ok
  IfFileExists "$PROGRAMFILES64\Git\bin\bash.exe" k2_git_ok
  MessageBox MB_YESNO|MB_ICONINFORMATION "K2's command-line tools need Git for Windows (Git Bash).$\r$\n$\r$\nK2 itself runs without it, but agents can't use the k2 command until it's installed.$\r$\n$\r$\nOpen the download page (https://git-scm.com/download/win) now?" /SD IDNO IDNO k2_git_ok
  ExecShell "open" "https://git-scm.com/download/win"
  k2_git_ok:
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog 'taskkill /F /IM k2.exe /T'
  nsExec::ExecToLog 'taskkill /F /IM k2-daemon.exe /T'
  nsExec::ExecToLog 'taskkill /F /IM frpc.exe /T'
  Sleep 800
  ; The daemon's logon autostart (k2_client::win_autostart).
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "K2 Daemon"
!macroend
