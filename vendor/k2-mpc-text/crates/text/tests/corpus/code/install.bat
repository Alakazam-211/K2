@echo off
rem Install the tool for the current user.
setlocal
set TARGET=%LOCALAPPDATA%\Tool
if not exist "%TARGET%" mkdir "%TARGET%"
copy /Y tool.exe "%TARGET%\tool.exe" >nul
echo Installed to %TARGET%
endlocal
