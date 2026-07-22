#!/bin/sh
grep -q 'debug: false' config.prod.yaml && grep -q 'debug: true' config.dev.yaml
