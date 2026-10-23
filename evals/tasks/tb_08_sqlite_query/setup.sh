#!/bin/bash
python3 - <<'EOF'
import sqlite3
conn = sqlite3.connect("store.db")
conn.execute("CREATE TABLE products (name TEXT, price REAL)")
conn.execute("INSERT INTO products VALUES ('Widget', 9.99)")
conn.execute("INSERT INTO products VALUES ('Gadget', 49.99)")
conn.execute("INSERT INTO products VALUES ('Doohickey', 24.99)")
conn.commit()
conn.close()
EOF
