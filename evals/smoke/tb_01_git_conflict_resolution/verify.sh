#!/usr/bin/env bash
set -euo pipefail
# Verify: file.txt has both lines, no conflict markers, and a commit msg 'resolved'
grep -q "line2 main" file.txt
grep -q "line2 feature" file.txt
! grep -q "<<<<<<" file.txt
! grep -q "=======" file.txt
! grep -q ">>>>>>>" file.txt
git log --oneline | grep "resolved" > /dev/null
