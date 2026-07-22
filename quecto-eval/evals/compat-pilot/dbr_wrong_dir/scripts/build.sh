#!/bin/sh
if [ ! -f "package.json" ]; then
  echo "Must be run from project root"
  exit 1
fi
touch build.ok
