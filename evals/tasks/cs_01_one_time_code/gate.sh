#!/usr/bin/env bash
set -euo pipefail
test -s out/final.txt
test -f a.txt && test -f b.txt && test -f c.txt
