#!/bin/sh
# Fake `diff` for hermetic svn synthetic-addition tests.
echo "$*" >> .fakediff.log
if [ -e .fakediff-empty ]; then
  exit 0
fi
if [ -e .fakediff-canned ]; then
  cat .fakediff-canned
fi
exit 1
