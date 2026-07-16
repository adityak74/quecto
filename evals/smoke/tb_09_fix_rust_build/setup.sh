#!/bin/bash
mkdir -p src
cat > Cargo.toml <<'EOF'
[package]
name = "summer"
version = "0.1.0"
edition = "2021"
EOF

# Deliberately broken: missing mut, wrong range, wrong format string
cat > src/main.rs <<'EOF'
fn main() {
    let sum = 0;
    for i in 1..=10 {
        sum += i;   // error: cannot assign to immutable
    }
    println!("total={}", sum);  // wrong: should print "sum=55"
}
EOF
