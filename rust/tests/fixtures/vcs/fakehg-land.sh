#!/bin/sh
# Fake `hg` for `arc land` seam tests (L10). Logs argv to .fakehg.log,
# answers from .fakehg-* marker files, and honors .fakehg-fail-<sub>
# failure markers. Subcommands not listed below succeed silently.
raw="$*"
echo "$raw" >> ./.fakehg.log
if [ "$1" = "--encoding" ]; then shift 2; fi
if [ "$1" = "--config" ]; then shift 2; fi
sub="$1"
if [ -z "$sub" ]; then exit 0; fi
if [ -e "./.fakehg-fail-$sub" ]; then exit 1; fi
NODE='0123456789abcdef0123456789abcdef01234567'
TIP='89abcdef0123456789abcdef0123456789abcdef01'
emit() { if [ -e "$1" ]; then cat "$1"; fi; }
emit_or() { if [ -e "$1" ]; then cat "$1"; else printf '%s' "$2"; fi; }
case "$sub" in
  branch)
    printf 'default\n';;
  branches)
    emit ./.fakehg-branches;;
  bookmarks)
    if [ -e ./.fakehg-bookmarks ]; then emit ./.fakehg-bookmarks;
    else printf ' * feature  4:aaaaaaaabbbbccccddddeeeeffff0000111122223333\n'; fi;;
  status)
    emit ./.fakehg-status;;
  paths)
    if [ ! -e ./.fakehg-nopaths ]; then printf 'https://hg.example.test/repo\n'; fi;;
  identify)
    printf '%s\n' "$NODE";;
  log)
    case "$raw" in
      *'parents %'*) emit ./.fakehg-log-commits;;
      *'-r .') emit_or ./.fakehg-node "$NODE";;
      *'--rev tip'*) emit_or ./.fakehg-tip "$TIP";;
      *'{desc}'*)
        if [ -e ./.fakehg-msg ]; then emit ./.fakehg-msg; else printf 'old message\n'; fi;;
      *'children('*) emit ./.fakehg-children;;
      *'nope'*) exit 1;;
      *) printf '%s' "$NODE";;
    esac;;
  rebase)
    if [ ! -t 0 ]; then cat >/dev/null; fi;;
  *)
    ;;
esac
exit 0
