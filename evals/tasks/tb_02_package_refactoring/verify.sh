#!/usr/bin/env bash
set -euo pipefail
# Verify: package structure and runnable module
test -f app/__init__.py
test -f app/cli.py
test -f app/core/__init__.py
test -f app/core/utils.py
test -f app/core/config.py
python3 -m app.cli
