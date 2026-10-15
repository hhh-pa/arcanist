#!/bin/sh
# Fake `hg` for hermetic tests. Logs argv, then prints canned output.
echo "$*" >> .fakehg.log
if [ "$1" = "--encoding" ]; then
  shift
  shift
fi
SUB="$1"
if [ -z "$SUB" ]; then
  exit 0
fi
if [ -e ".fakehg-fail-$SUB" ]; then
  exit 1
fi
NODE="0123456789abcdef0123456789abcdef01234567"
case "$SUB" in
  branch)
    echo "default"
    ;;
  status)
    if [ -e .fakehg-status ]; then
      cat .fakehg-status
    fi
    ;;
  log)
    case "$*" in
      *children\(\.\)*)
        if [ -e .fakehg-children ]; then
          cat .fakehg-children
        fi
        ;;
      *"{rev}"*)
        if [ -e .fakehg-log-info ]; then
          cat .fakehg-log-info
        fi
        ;;
      *nope*)
        exit 1
        ;;
      *"::. -"*)
        if [ -e .fakehg-log-msg ]; then
          cat .fakehg-log-msg
        fi
        ;;
      *"::."*)
        if [ -e .fakehg-log-nodiff ]; then
          cat .fakehg-log-nodiff
        fi
        ;;
      *--branch*)
        if [ -e .fakehg-log-draft ]; then
          cat .fakehg-log-draft
        fi
        ;;
      *outgoing\(\)*|*bookmark\(\)*|*ancestor\(*)
        printf '%s' "$NODE"
        ;;
      *parents\(draft\(\)\)*)
        if [ -e .fakehg-log-pub ]; then
          cat .fakehg-log-pub
        fi
        ;;
      *"{desc}"*)
        if [ -e .fakehg-msg ]; then
          cat .fakehg-msg
        else
          echo "old message"
        fi
        ;;
      *)
        printf '%s' "$NODE"
        ;;
    esac
    ;;
  parents)
    case "$*" in
      *"{node}"*)
        if [ -e .fakehg-parents-tpl ]; then
          cat .fakehg-parents-tpl
        fi
        ;;
      *)
        if [ -e .fakehg-parents ]; then
          cat .fakehg-parents
        fi
        ;;
    esac
    ;;
  manifest)
    if [ -e .fakehg-manifest ]; then
      cat .fakehg-manifest
    fi
    ;;
  annotate)
    if [ -e .fakehg-annotate ]; then
      cat .fakehg-annotate
    fi
    ;;
  cat)
    case "$*" in
      *--output*) ;;
      *absent*) exit 1 ;;
      *)
        if [ -e .fakehg-cat ]; then
          cat .fakehg-cat
        fi
        ;;
    esac
    ;;
  showconfig)
    echo "Test User <t@example.test>"
    ;;
  help)
    if [ -e .fakehg-help ]; then
      cat .fakehg-help
    elif [ -e .fakehg-noamend ]; then
      echo "no flags here"
    else
      echo "hg commit [OPTION]..."
      echo "  --amend flag"
    fi
    ;;
  paths)
    if [ ! -e .fakehg-nopaths ]; then
      echo "https://hg.example.test/repo"
    fi
    ;;
  envprobe)
    echo "HGPLAIN=$HGPLAIN"
    ;;
  svn)
    if [ -e .fakehg-svninfo ]; then
      cat .fakehg-svninfo
    fi
    ;;
  diff)
    if [ -e .fakehg-diff ]; then
      cat .fakehg-diff
    fi
    ;;
esac
exit 0
