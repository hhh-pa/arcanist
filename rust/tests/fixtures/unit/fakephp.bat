@echo off
rem Fake `php` for hermetic phutil-engine tests: logs argv, snapshots the
rem staged input, then writes canned JSON output (or fails) like the real
rem driver protocol. Usage: fakephp.bat <driver> <input.json> <output.json>.
echo %*>> .fakephp.log
copy /y "%~2" .fakephp-input-seen >nul 2>&1
set OUT=%~3
if exist .fakephp-fail (
  echo fake php exploded 1>&2
  exit /b 3
)
if exist .fakephp-output (
  copy /y .fakephp-output "%OUT%" >nul
) else (
  echo {"results":[]}>"%OUT%"
)
exit /b 0
