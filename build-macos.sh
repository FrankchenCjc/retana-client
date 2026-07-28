#!/bin/bash
# retana — Build for macOS 26+ Apple Silicon (aarch64)
#
# Prerequisites (on macOS):
#   brew install rustup
#   rustup target add aarch64-apple-darwin
#   xcode-select --install
#
# Then:
#   npm install
#   npm run tauri build -- --target aarch64-apple-darwin

set -e

echo "🔨 Building retana for macOS 26+ (Apple Silicon)"
echo ""

# Ensure target is installed
rustup target list --installed | grep -q aarch64-apple-darwin || {
    echo "📦 Installing aarch64-apple-darwin target..."
    rustup target add aarch64-apple-darwin
}

# Build frontend
echo "📦 Building frontend..."
npm run build

# Build Tauri app
echo "🦀 Building Tauri app for aarch64-apple-darwin..."
cd src-tauri
cargo build --release --target aarch64-apple-darwin
cd ..

echo ""
echo "✅ Build complete!"
echo "   Binary: src-tauri/target/aarch64-apple-darwin/release/retana"
echo "   Bundle: src-tauri/target/aarch64-apple-darwin/release/bundle/"
