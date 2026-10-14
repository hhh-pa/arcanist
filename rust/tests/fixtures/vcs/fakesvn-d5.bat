@echo off
rem Fake `svn` for per-path fan-out tests (D5/D11). Logs argv, then prints
rem canned per-path output: `.fakesvn-d5-status` for status,
rem `.fakesvn-d5-info-<rel>` for info (relative to cwd, separators as `_`,
rem the working-copy root as `root`), `.fakesvn-d5-diff-<rel>` for diff.
set "ARGS=%*"
echo %ARGS%>> .fakesvn-d5.log
set "SUB=%~1"
if "%SUB%"=="" exit /b 0
if "%SUB%"=="--xml" set "SUB=%~2"
if "%SUB%"=="status" goto d5_status
if "%SUB%"=="info" goto d5_info
if "%SUB%"=="diff" goto d5_diff
if "%SUB%"=="propget" goto d5_propget
if "%SUB%"=="propset" exit /b 0
if "%SUB%"=="cat" goto d5_cat
exit /b 0
:d5_status
if exist .fakesvn-d5-status type .fakesvn-d5-status
if not exist .fakesvn-d5-status echo ^<status/^>
exit /b 0
:d5_info
set "LAST="
:info_loop
if "%~1"=="" goto info_done
set "LAST=%~1"
shift
goto info_loop
:info_done
set "ARG=%LAST%"
if "%ARG:~-1%"=="@" set "ARG=%ARG:~0,-1%"
call set "REL=%%ARG:%CD%\=%%"
if "%REL%"=="." set "REL=root"
set "SAN=%REL:/=_%"
set "SAN=%SAN:\=_%"
if exist ".fakesvn-d5-info-%SAN%" type ".fakesvn-d5-info-%SAN%"
exit /b 0
:d5_diff
set "LAST="
:diff_loop
if "%~1"=="" goto diff_done
set "LAST=%~1"
shift
goto diff_loop
:diff_done
set "SAN=%LAST:/=_%"
set "SAN=%SAN:\=_%"
if exist ".fakesvn-d5-diff-%SAN%" type ".fakesvn-d5-diff-%SAN%"
exit /b 0
:d5_propget
echo text/plain
exit /b 0
:d5_cat
if exist .fakesvn-d5-cat type .fakesvn-d5-cat
if not exist .fakesvn-d5-cat <nul set /p =file-bytes
exit /b 0
