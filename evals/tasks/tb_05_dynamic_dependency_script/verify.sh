#!/usr/bin/env bash
set -euo pipefail
# Verify: scraper.py exists, has BeautifulSoup + try/except, output.txt has correct items
test -f scraper.py
grep -q "BeautifulSoup" scraper.py
grep -q "ImportError" scraper.py
test -f output.txt
grep -q "Item1" output.txt
grep -q "Item2" output.txt
# The "Ignore" item from the non-target ul must NOT be in output
! grep -q "Ignore" output.txt
