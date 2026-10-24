#!/usr/bin/env bash
set -euo pipefail
grep -qx "# Changelog" CHANGELOG.md
grep -Eq -- "^- ticket T-.{5}$" CHANGELOG.md
