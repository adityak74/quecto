#!/bin/sh
set -e
cat > setup-db.sh <<'EOF'
#!/bin/sh
touch db.sqlite
EOF
