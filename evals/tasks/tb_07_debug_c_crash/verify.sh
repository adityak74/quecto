#!/usr/bin/env bash
set -euo pipefail
# Verify: fixed C program compiles and runs correctly
gcc crasher.c -o crasher -g
output=$(./crasher)
[ "$output" = "OK" ]
