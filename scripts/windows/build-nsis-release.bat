@echo off
REM Sticky-box Windows NSIS release build (layout contract: C:\k2\K2 + C:\k2\K2-target).
REM Invoked by scripts/windows-nsis-build.sh via SSH. Do not hardcode LAN hosts here.
setlocal
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
echo === cargo tauri build nsis ===
cargo tauri build --bundles nsis --config "{\"build\":{\"beforeBuildCommand\":\"\"}}"
if errorlevel 1 exit /b 1
echo ALL_OK
exit /b 0
