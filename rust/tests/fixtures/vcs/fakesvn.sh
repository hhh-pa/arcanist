#!/bin/sh
# Fake `svn` for hermetic tests. Logs argv, then prints canned output.
echo "$*" >> .fakesvn.log
SUB="$1"
if [ "$SUB" = "--xml" ]; then
  SUB="$2"
fi
if [ -z "$SUB" ]; then
  exit 0
fi
if [ -e ".fakesvn-fail-$SUB" ]; then
  exit 1
fi
case "$SUB" in
  status)
    if [ -e .fakesvn-status ]; then
      cat .fakesvn-status
    else
      echo "<status/>"
    fi
    ;;
  info)
    if [ -e .fakesvn-info ]; then
      cat .fakesvn-info
    fi
    ;;
  blame)
    if [ -e .fakesvn-blame ]; then
      cat .fakesvn-blame
    fi
    ;;
  propget)
    if [ -e .fakesvn-propget ]; then
      cat .fakesvn-propget
    else
      echo "text/plain"
    fi
    ;;
  list)
    if [ -e .fakesvn-list ]; then
      cat .fakesvn-list
    fi
    ;;
  cat)
    case "$*" in
      *http*)
        if [ -e .fakesvn-cat-url ]; then
          cat .fakesvn-cat-url
        else
          printf '%s' "url-bytes"
        fi
        ;;
      *absent*)
        exit 1
        ;;
      *)
        if [ -e .fakesvn-cat ]; then
          cat .fakesvn-cat
        else
          printf '%s' "file-bytes"
        fi
        ;;
    esac
    ;;
  diff)
    case "$*" in
      *--summarize*)
        if [ -e .fakesvn-diffsumm ]; then
          cat .fakesvn-diffsumm
        fi
        ;;
      *)
        if [ -e .fakesvn-diff2 ]; then
          if [ -e .fakesvn-diff ]; then
            cat .fakesvn-diff
          fi
          if [ -e .fakesvn-diff2err ]; then
            cat .fakesvn-diff2err 1>&2
          else
            echo "svn: 'diff' returned 2" 1>&2
          fi
          exit 1
        fi
        if [ -e .fakesvn-fail-diff ]; then
          exit 1
        fi
        if [ -e .fakesvn-diff ]; then
          cat .fakesvn-diff
        fi
        ;;
    esac
    ;;
esac
exit 0
