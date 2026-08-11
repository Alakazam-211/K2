@echo off
REM Sticky-box Windows NSIS release build (layout: C:\k2\K2 + C:\k2\K2-target).
REM Mirrors macOS release.sh Step 2.5: ship k2.exe + k2-daemon.exe + frpc.exe.
REM Invoked by scripts/windows-nsis-build.sh. Do not hardcode LAN hosts here.
setlocal EnableExtensions
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" || exit /b 1
set "PATH=%USERPROFILE%\.bun\bin;%USERPROFILE%\.cargo\bin;C:\Program Files\Git\cmd;C:\Program Files\CMake\bin;C:\k2\LLVM\bin;C:\Program Files (x86)\NSIS;C:\Users\rosso\AppData\Local\Microsoft\WinGet\Links;%PATH%"
set "LIBCLANG_PATH=C:\k2\LLVM\bin"
set "CMAKE_GENERATOR=Ninja"
if not defined CARGO_TARGET_DIR set "CARGO_TARGET_DIR=C:\k2\K2-target"
if not defined K2_WIN_TREE set "K2_WIN_TREE=C:\k2\K2"
cd /d "%K2_WIN_TREE%" || exit /b 1

echo CARGO_TARGET_DIR=%CARGO_TARGET_DIR%
echo K2_WIN_TREE=%K2_WIN_TREE%

echo === bun install ===
call bun install --frozen-lockfile
if errorlevel 1 (
  echo frozen-lockfile failed — retrying plain bun install
  call bun install
)
if errorlevel 1 exit /b 1

echo === vite:build ===
call bun run vite:build
if errorlevel 1 exit /b 1

REM ── Step 2.5 equivalent: build + stage k2-daemon as Tauri externalBin ──
REM Tauri only packs the primary bin + externalBin; same as macOS where we
REM cp k2-daemon into Contents/MacOS after tauri build. On Windows we stage
REM BEFORE nsis so the installer ships daemon next to k2.exe.
echo === cargo build --release -p k2-daemon ===
cargo build --release -p k2-daemon
if errorlevel 1 exit /b 1

set "DAEMON_SRC=%CARGO_TARGET_DIR%\release\k2-daemon.exe"
if not exist "%DAEMON_SRC%" (
  echo FATAL: k2-daemon.exe missing at %DAEMON_SRC%
  exit /b 1
)

set "BINARIES=%K2_WIN_TREE%\src-tauri\binaries"
if not exist "%BINARIES%" mkdir "%BINARIES%"

REM Triple-suffixed name required by Tauri externalBin convention.
set "DAEMON_SIDE=%BINARIES%\k2-daemon-x86_64-pc-windows-msvc.exe"
copy /Y "%DAEMON_SRC%" "%DAEMON_SIDE%" >nul
if errorlevel 1 exit /b 1
echo staged k2-daemon -^> %DAEMON_SIDE%

set "FRPC_SIDE=%BINARIES%\frpc-x86_64-pc-windows-msvc.exe"
if not exist "%FRPC_SIDE%" (
  echo FATAL: frpc sidecar missing at %FRPC_SIDE%
  echo Stage with scripts/fetch-frpc.sh FRPC_TARGET_TRIPLE=x86_64-pc-windows-msvc FRPC_SRC=...
  exit /b 1
)
echo frpc sidecar OK: %FRPC_SIDE%

echo === cargo tauri build --bundles nsis ===
REM beforeBuildCommand emptied — vite already ran. externalBin for daemon
REM comes from tauri.windows.conf.json (frpc + k2-daemon).
cargo tauri build --bundles nsis --config "{\"build\":{\"beforeBuildCommand\":\"\"}}"
if errorlevel 1 exit /b 1

REM Prove the three peers landed next to each other in the release dir
REM (NSIS packs from here / tauri's bundle layout).
echo === verify peer binaries ===
set "REL=%CARGO_TARGET_DIR%\release"
if not exist "%REL%\k2.exe" (
  echo FATAL: k2.exe missing
  exit /b 1
)
if not exist "%REL%\k2-daemon.exe" (
  echo FATAL: k2-daemon.exe not next to k2 after tauri build
  exit /b 1
)
if not exist "%REL%\frpc.exe" (
  echo FATAL: frpc.exe not next to k2 after tauri build
  exit /b 1
)
dir "%REL%\k2.exe" "%REL%\k2-daemon.exe" "%REL%\frpc.exe"

set "NSIS_DIR=%REL%\bundle\nsis"
dir /b "%NSIS_DIR%\K2_*_x64-setup.exe" >nul 2>&1
if errorlevel 1 (
  echo FATAL: NSIS installer missing under %NSIS_DIR%
  dir "%NSIS_DIR%"
  exit /b 1
)
dir "%NSIS_DIR%\*.exe"

echo ALL_OK
echo BUNDLE_PEERS=k2.exe+k2-daemon.exe+frpc.exe
exit /b 0
