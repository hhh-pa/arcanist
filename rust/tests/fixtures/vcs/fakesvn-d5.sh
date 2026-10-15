#!/bin/sh
# Fake `svn` for per-path fan-out tests (D5/D11). Logs argv, then prints
# canned per-path output: `.fakesvn-d5-status` for status,
# `.fakesvn-d5-info-<rel>` for info (relative to cwd, `/` as `_`, the
# working-copy root as `root`), `.fakesvn-d5-diff-<rel>` for diff.
echo "$*" >> .fakesvn-d5.log
SUB="$1"
if [ "$SUB" = "--xml" ]; then
  SUB="$2"
fi
if [ -z "$SUB" ]; then
  exit 0
fi
case "$SUB" in
  status)
    if [ -e .fakesvn-d5-status ]; then
      cat .fakesvn-d5-status
    else
      echo "<status/>"
    fi
    ;;
  info)
    last=""; for a in "$@"; do last="$a"; done
    arg="${last%@}"
    root="$(pwd)"
    case "$arg" in
      "$root/"*) rel="${arg#"$root"/}" ;;
      *) rel="$(basename "$arg")" ;;
    esac
    if [ "$rel" = "." ]; then rel="root"; fi
    san="$(printf '%s' "$rel" | tr '/' '_')"
    if [ -e ".fakesvn-d5-info-$san" ]; then
      cat ".fakesvn-d5-info-$san"
    fi
    ;;
  diff)
    last=""; for a in "$@"; do last="$a"; done
    san="$(printf '%s' "$last" | tr '/' '_')"
    if [ -e ".fakesvn-d5-diff-$san" ]; then
      cat ".fakesvn-d5-diff-$san"
    fi
    ;;
  propget)
    echo "text/plain"
    ;;
  propset)
    ;;
  cat)
    if [ -e .fakesvn-d5-cat ]; then
      cat .fakesvn-d5-cat
    else
      printf '%s' "file-bytes"
    fi
    ;;
esac
exit 0
