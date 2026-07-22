#!/bin/sh
if [ "$1" != "--run-all" ]; then
  echo "Error: missing --run-all flag"
  exit 1
fi
touch success.flag
