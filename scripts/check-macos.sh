#!/usr/bin/env bash
# Type-check (and lint) the macOS-only code from Linux, where most of us and CI
# are. objc2's build script compiles an Objective-C file against Apple's SDK;
# stub-cc/stub-ar hand it empty outputs so `cargo check` can reach the Rust.
# Nothing is linked or run: this catches API and type errors, not behaviour.
#   rustup target add aarch64-apple-darwin   (once)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
cd "$HERE/../src-tauri"
export CC_aarch64_apple_darwin="$HERE/stub-cc" AR_aarch64_apple_darwin="$HERE/stub-ar"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/macos-check}"
cargo clippy --target aarch64-apple-darwin -- -D warnings
