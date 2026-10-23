#!/usr/bin/env bash
set -euo pipefail
python3 - <<'E'
from utils import slugify
assert slugify("Hello, World!")=="hello-world"
assert slugify("  A  b__c ")=="a-b-c"
assert slugify("---")==""
assert slugify("x")=="x"
assert slugify("Rust 2021 Edition")=="rust-2021-edition"
E
