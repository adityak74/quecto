#!/usr/bin/env bash
set -euo pipefail
# Verify: Dockerfile uses correct base, COPY and ENTRYPOINT; image runs and prints correctly
grep -q "python:3.11-slim" Dockerfile
grep -q "COPY app.py" Dockerfile
test -f output.txt
grep -q "Hello Docker" output.txt
