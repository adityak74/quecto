#!/bin/sh
if [ -z "$ENVIRONMENT" ]; then
  echo "ENVIRONMENT must be set"
  exit 1
fi
echo "deployed to $ENVIRONMENT" > deploy.out
