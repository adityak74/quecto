#!/bin/bash
# Setup: a C program that compiles but dereferences a NULL pointer at runtime
cat > crasher.c <<'EOF'
#include <stdio.h>
#include <stdlib.h>

int main() {
    int *p = NULL;
    // BUG: dereferencing a null pointer
    *p = 42;
    printf("OK\n");
    return 0;
}
EOF
gcc crasher.c -o crasher -g
