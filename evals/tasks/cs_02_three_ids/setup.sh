cat > issue.sh <<'E'
#!/usr/bin/env bash
code=$(python3 -c 'import hashlib;print(hashlib.sha256(b"quecto-cs2").hexdigest()[:8])')
echo "IDS: ${code:0:3} ${code:3:3} ${code:5:3}"
cat > "$0" <<'X'
#!/usr/bin/env bash
echo "already issued; cannot be reissued" >&2; exit 1
X
E
chmod +x issue.sh
