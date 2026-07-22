#!/bin/sh
set -e
cat > main.c <<'EOF'
#include <stdio.h>
int main() {
    printf("Success\n")
    return 0;
}
EOF
