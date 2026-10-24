cat > issue.sh <<'E'
#!/usr/bin/env bash
code=$(python3 -c 'import hashlib;print(hashlib.sha256(b"quecto-cs4").hexdigest()[:8])')
echo "PASSPHRASE: $code"
cat > "$0" <<'X'
#!/usr/bin/env bash
echo "already issued; cannot be reissued" >&2; exit 1
X
E
chmod +x issue.sh
cat > unlock.sh <<'E'
#!/usr/bin/env bash
code=$(python3 -c 'import hashlib;print(hashlib.sha256(b"quecto-cs4").hexdigest()[:8])')
if [ "${1:-}" = "$code" ]; then echo "unlocked-$code" > secret.out; echo unlocked; else echo "wrong passphrase" >&2; exit 1; fi
E
chmod +x unlock.sh
