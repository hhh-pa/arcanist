@echo off
rem Fake `svn` for hermetic tests. Logs argv, then prints canned output.
set "ARGS=%*"
echo %ARGS%>> .fakesvn.log
set "SUB=%~1"
if "%SUB%"=="" exit /b 0
if "%SUB%"=="--xml" set "SUB=%~2"
if exist ".fakesvn-fail-%SUB%" exit /b 1
if "%SUB%"=="status" goto svn_status
if "%SUB%"=="info" goto svn_info
if "%SUB%"=="blame" goto svn_blame
if "%SUB%"=="propget" goto svn_propget
if "%SUB%"=="list" goto svn_list
if "%SUB%"=="cat" goto svn_cat
if "%SUB%"=="diff" goto svn_diff
exit /b 0
:svn_status
if exist .fakesvn-status type .fakesvn-status
if not exist .fakesvn-status echo ^<status/^>
exit /b 0
:svn_info
if exist .fakesvn-info type .fakesvn-info
exit /b 0
:svn_blame
if exist .fakesvn-blame type .fakesvn-blame
exit /b 0
:svn_propget
if exist .fakesvn-propget type .fakesvn-propget
if not exist .fakesvn-propget echo text/plain
exit /b 0
:svn_list
if exist .fakesvn-list type .fakesvn-list
exit /b 0
:svn_cat
echo %ARGS% | findstr /C:"http" >nul
if not errorlevel 1 goto svn_caturl
echo %ARGS% | findstr /C:"absent" >nul
if not errorlevel 1 exit /b 1
if exist .fakesvn-cat type .fakesvn-cat
if not exist .fakesvn-cat <nul set /p =file-bytes
exit /b 0
:svn_caturl
if exist .fakesvn-cat-url type .fakesvn-cat-url
if not exist .fakesvn-cat-url <nul set /p =url-bytes
exit /b 0
:svn_diff
echo %ARGS% | findstr /C:"--summarize" >nul
if not errorlevel 1 goto svn_diffsumm
if exist .fakesvn-diff2 goto svn_diff2
if exist .fakesvn-fail-diff exit /b 1
if exist .fakesvn-diff type .fakesvn-diff
exit /b 0
:svn_diff2
if exist .fakesvn-diff type .fakesvn-diff
if exist .fakesvn-diff2err (type .fakesvn-diff2err 1>&2) else (echo svn: 'diff' returned 2 1>&2)
exit /b 1
:svn_diffsumm
if exist .fakesvn-diffsumm type .fakesvn-diffsumm
exit /b 0
