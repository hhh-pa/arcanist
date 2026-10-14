#!/bin/sh
# Fake `php` for hermetic phutil-engine tests: logs argv, snapshots the
# staged input, then writes canned JSON output (or fails) like the real
# driver protocol. Usage: fakephp.sh <driver> <input.json> <output.json>.
echo "$*" >> .fakephp.log
cp "$2" .fakephp-input-seen 2>/dev/null
OUT="$3"
if [ -e .fakephp-fail ]; then
  echo "fake php exploded" 1>&2
  exit 3
fi
if [ -e .fakephp-output ]; then
  cp .fakephp-output "$OUT"
else
  printf '%s' '{"results":[]}' > "$OUT"
fi
exit 0
