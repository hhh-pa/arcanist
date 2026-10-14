@echo off
rem Fake `hg` for bookmark/branch listing tests. Logs argv, then prints canned output.
set "ARGS=%*"
echo %ARGS%>> .fakehg.log
set "SUB=%~1"
if "%SUB%"=="--encoding" set "SUB=%~3"
if exist ".fakehg-fail-%SUB%" exit /b 1
if "%SUB%"=="bookmarks" goto hg_bookmarks
if "%SUB%"=="branches" goto hg_branches
if "%SUB%"=="branch" goto hg_branch
if "%SUB%"=="log" goto hg_log
exit /b 0
:hg_bookmarks
if exist .fakehg-bookmarks type .fakehg-bookmarks
exit /b 0
:hg_branches
if exist .fakehg-branches type .fakehg-branches
exit /b 0
:hg_branch
if exist .fakehg-branch type .fakehg-branch
if not exist .fakehg-branch <nul set /p =default
exit /b 0
:hg_log
if exist .fakehg-msg type .fakehg-msg
exit /b 0
