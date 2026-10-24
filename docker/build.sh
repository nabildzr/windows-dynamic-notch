#!/bin/sh
# Runs inside notch-builder: cross-compiles for x86_64-pc-windows-gnu and
# copies the executable to ./dist.
#   build.sh [release|devrel|check|clippy|test]
set -e
cd /src
TARGET=x86_64-pc-windows-gnu
MODE=${1:-release}

case "$MODE" in
  check)
    cargo check --target "$TARGET" --all-targets
    ;;
  clippy)
    cargo clippy --target "$TARGET" --all-targets -- -D warnings
    ;;
  test)
    # Builds the unit-test executable for Windows; build.ps1 runs it on the host.
    OUT=$(cargo test --no-run --target "$TARGET" 2>&1)
    echo "$OUT" | tail -3
    EXE=$(echo "$OUT" | sed -n 's/.*Executable unittests src\/main.rs (\(.*\.exe\)).*/\1/p' | head -1)
    mkdir -p dist
    cp "$EXE" dist/notch-tests.exe
    ;;
  installer)
    # Release build + NSIS setup executable in dist/.
    cargo build --profile release --target "$TARGET"
    mkdir -p dist
    cp "/target/$TARGET/release/windows-dynamic-notch.exe" dist/windows-dynamic-notch.exe
    VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
    makensis -V2 -DAPP_VERSION="$VERSION" installer/windows-dynamic-notch.nsi
    7z l "dist/WindowsDynamicNotch-Setup-$VERSION.exe" | tail -15
    ls -la dist
    ;;
  *)
    cargo build --profile "$MODE" --target "$TARGET"
    mkdir -p dist
    cp "/target/$TARGET/$MODE/windows-dynamic-notch.exe" "dist/windows-dynamic-notch.exe"
    ls -la dist/windows-dynamic-notch.exe
    ;;
esac
