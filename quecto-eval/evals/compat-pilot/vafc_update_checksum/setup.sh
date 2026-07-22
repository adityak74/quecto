#!/bin/sh
set -e
echo '{"version": "1.0"}' > config.json
cat > update_hash.sh <<'EOF'
#!/bin/sh
md5sum config.json > hash.md5
EOF
chmod +x update_hash.sh
./update_hash.sh
