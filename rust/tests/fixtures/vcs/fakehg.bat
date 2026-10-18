@echo off
rem Fake `hg` for hermetic tests. Logs argv, then prints canned output.
rem NOTE: cmd.exe mangles shell-special argv (`<`, `>`, `|`, `&`) when
rem logging/forwarding; tests with such argv must use fakehg.ps1 instead.
set "ARGS=%*"
echo %ARGS%>> .fakehg.log
if "%~1"=="--encoding" (
  shift
  shift
)
set "SUB=%~1"
if "%SUB%"=="" exit /b 0
if exist ".fakehg-fail-%SUB%" exit /b 1
if "%SUB%"=="branch" echo default& exit /b 0
if "%SUB%"=="status" goto hg_status
if "%SUB%"=="log" goto hg_log
if "%SUB%"=="parents" goto hg_parents
if "%SUB%"=="manifest" goto hg_manifest
if "%SUB%"=="annotate" goto hg_annotate
if "%SUB%"=="cat" goto hg_cat
if "%SUB%"=="showconfig" echo Test User ^<t@example.test^>& exit /b 0
if "%SUB%"=="help" goto hg_help
if "%SUB%"=="paths" goto hg_paths
if "%SUB%"=="svn" goto hg_svn
if "%SUB%"=="diff" goto hg_diff
if "%SUB%"=="envprobe" echo HGPLAIN=%HGPLAIN%& exit /b 0
exit /b 0
:hg_status
if exist .fakehg-status type .fakehg-status
exit /b 0
:hg_log
echo %ARGS% | findstr /C:"children(.)" >nul
if not errorlevel 1 goto hg_children
echo %ARGS% | findstr /C:"{rev}" >nul
if not errorlevel 1 goto hg_loginforev
echo %ARGS% | findstr /C:"nope" >nul
if not errorlevel 1 exit /b 1
echo %ARGS% | findstr /C:"::. -" >nul
if not errorlevel 1 goto hg_logmsg
rem Nodiff revsets end "::.^" but cmd.exe eats carets, so match "::." here
rem after the "::. -" message-log check above.
echo %ARGS% | findstr /C:"::." >nul
if not errorlevel 1 goto hg_lognodiff
echo %ARGS% | findstr /C:"--branch" >nul
if not errorlevel 1 goto hg_logdraft
echo %ARGS% | findstr /C:"outgoing()" >nul
if not errorlevel 1 goto hg_node
echo %ARGS% | findstr /C:"bookmark()" >nul
if not errorlevel 1 goto hg_node
echo %ARGS% | findstr /C:"parents(draft())" >nul
if not errorlevel 1 goto hg_logpub
echo %ARGS% | findstr /C:"ancestor(" >nul
if not errorlevel 1 goto hg_node
echo %ARGS% | findstr /C:"{desc}" >nul
if not errorlevel 1 goto hg_msg
goto hg_node
:hg_children
if exist .fakehg-children type .fakehg-children
exit /b 0
:hg_loginforev
if exist .fakehg-log-info type .fakehg-log-info
exit /b 0
:hg_logmsg
if exist .fakehg-log-msg type .fakehg-log-msg
exit /b 0
:hg_lognodiff
if exist .fakehg-log-nodiff type .fakehg-log-nodiff
exit /b 0
:hg_logdraft
if exist .fakehg-log-draft type .fakehg-log-draft
exit /b 0
:hg_logpub
if exist .fakehg-log-pub type .fakehg-log-pub
exit /b 0
:hg_msg
if exist .fakehg-msg type .fakehg-msg
if not exist .fakehg-msg echo old message
exit /b 0
:hg_node
<nul set /p =0123456789abcdef0123456789abcdef01234567
exit /b 0
:hg_parents
echo %ARGS% | findstr /C:"{node}" >nul
if not errorlevel 1 goto hg_parentstpl
if exist .fakehg-parents type .fakehg-parents
exit /b 0
:hg_parentstpl
if exist .fakehg-parents-tpl type .fakehg-parents-tpl
exit /b 0
:hg_manifest
if exist .fakehg-manifest type .fakehg-manifest
exit /b 0
:hg_annotate
if exist .fakehg-annotate type .fakehg-annotate
exit /b 0
:hg_cat
echo %ARGS% | findstr /C:"--output" >nul
if not errorlevel 1 exit /b 0
echo %ARGS% | findstr /C:"absent" >nul
if not errorlevel 1 exit /b 1
if exist .fakehg-cat type .fakehg-cat
exit /b 0
:hg_help
if exist .fakehg-help type .fakehg-help& exit /b 0
if exist .fakehg-noamend echo no flags here& exit /b 0
echo hg commit [OPTION]...
echo   --amend flag
exit /b 0
:hg_paths
if exist .fakehg-nopaths exit /b 0
echo https://hg.example.test/repo
exit /b 0
:hg_svn
if exist .fakehg-svninfo type .fakehg-svninfo
exit /b 0
:hg_diff
if exist .fakehg-diff type .fakehg-diff
exit /b 0
