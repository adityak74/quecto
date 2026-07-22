#!/bin/sh
set -e
echo 'helo' > src.txt
cat > build.sh <<'EOF'
#!/bin/sh
cp src.txt out.txt
EOF
chmod +x build.sh
