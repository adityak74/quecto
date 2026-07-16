#!/usr/bin/env bash
set -euo pipefail
cargo build --release 2>&1
output=$(./target/release/summer)
[ "$output" = "sum=55" ]
