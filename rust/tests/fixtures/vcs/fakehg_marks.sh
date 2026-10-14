#!/bin/sh
# Fake `hg` for bookmark/branch listing tests. Logs argv, then prints canned output.
echo "$*" >> .fakehg.log
SUB="$1"
if [ "$SUB" = "--encoding" ]; then shift 2; SUB="$1"; fi
if [ -f ".fakehg-fail-$SUB" ]; then exit 1; fi
case "$SUB" in
  bookmarks)
    if [ -e .fakehg-bookmarks ]; then cat .fakehg-bookmarks; fi
    ;;
  branches)
    if [ -e .fakehg-branches ]; then cat .fakehg-branches; fi
    ;;
  branch)
    if [ -e .fakehg-branch ]; then cat .fakehg-branch; else printf 'default'; fi
    ;;
  log)
    if [ -e .fakehg-msg ]; then cat .fakehg-msg; fi
    ;;
esac
exit 0
