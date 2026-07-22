#!/bin/sh
set -e
for i in $(seq 1 41); do echo "INFO: Line $i" >> app.log; done
echo "FATAL: Connection refused" >> app.log
for i in $(seq 43 50); do echo "INFO: Line $i" >> app.log; done
