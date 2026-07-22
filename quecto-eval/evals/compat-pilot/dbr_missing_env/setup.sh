#!/bin/sh
set -e
cat > deploy.sh <<'EOF'
#!/bin/sh
if [ -z "$ENVIRONMENT" ]; then
  echo "ENVIRONMENT must be set"
  exit 1
fi
echo "deployed to $ENVIRONMENT" > deploy.out
EOF
