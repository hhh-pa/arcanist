@echo off
rem Forwarder: PowerShell `-File` script binding rejects a bare `-` argv
rem (from `hg ... --logfile -`) with exit 1 before the script runs, so
rem rewrite just that token pair to `--logfile """-"""` (which arrives in
rem the script as `"-"`, unwrapped back to `-` there) and forward the raw
rem %* otherwise byte-identical. No token loop: re-emitting argv through
rem a variable re-parses it, and the commit-list template's quotes would
rem flip quote parity and expose its `|` as a real pipe. The `:old=new`
rem substitution keeps the original quoting, so every quoted token stays
rem self-contained and cmd metacharacters inside quotes stay literal.
set ALL=%*
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0fakehg-land.ps1" %ALL:--logfile -=--logfile """-"""%
