#!/bin/sh
grep -q '"2.0"' config.json && md5sum -c hash.md5 --status
