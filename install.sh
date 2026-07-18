#!/usr/bin/env bash
set -e

# Build the workspace binaries in release mode
echo "Cleaning the cargo workspace..."
cargo clean
echo "Building the quecto workspace binaries..."
cargo build --release --workspace

# Define the target installation directory
# We'll use ~/.local/bin as it's a standard user-level bin directory
INSTALL_DIR="$HOME/.local/bin"

echo "Creating $INSTALL_DIR if it doesn't exist..."
mkdir -p "$INSTALL_DIR"

echo "Copying binaries to $INSTALL_DIR..."
cp target/release/quecto "$INSTALL_DIR/"
cp target/release/quecto-agent "$INSTALL_DIR/"

echo "Successfully installed 'quecto' and 'quecto-agent' to $INSTALL_DIR!"

# Check if INSTALL_DIR is in PATH
if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo ""
    echo "======================================================================"
    echo "WARNING: $INSTALL_DIR is not in your PATH."
    echo "To attach these binaries to your PATH, add this line to your ~/.zshrc or ~/.bashrc:"
    echo ""
    echo "export PATH=\"\$PATH:$INSTALL_DIR\""
    echo "======================================================================"
    echo ""
    echo "After adding, run 'source ~/.zshrc' (or your shell config) to apply."
else
    echo "The binaries are already accessible in your PATH."
fi
