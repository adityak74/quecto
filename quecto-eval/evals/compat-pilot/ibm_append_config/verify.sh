#!/bin/sh
grep -q 'timeout: 30' settings.yaml && grep -q 'retries: 5' settings.yaml && grep -q 'mode: fast' settings.yaml
