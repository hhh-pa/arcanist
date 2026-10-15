@echo off
rem Fake `diff` for hermetic svn synthetic-addition tests.
echo %*>> .fakediff.log
if exist .fakediff-empty exit /b 0
if exist .fakediff-canned type .fakediff-canned
exit /b 1
