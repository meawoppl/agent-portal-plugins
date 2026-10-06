#!/bin/sh
# Build the kicadmium binary (with its embedded Yew frontend) for a plugin
# install, then print the binary's own `setup` report.
#
# Agent Portal runs manifest commands with HOME and XDG_CACHE_HOME pointed at
# <plugin>/.portal/{home,cache}. Left alone, rustup then downloads a complete
# toolchain, cargo re-fetches the crates.io index and every dependency, and
# trunk re-downloads wasm-bindgen and wasm-opt into the plugin directory on
# every install, after which everything compiles cold. That turns a three
# minute build into twenty. Point those tools at the user's existing caches
# instead; they only ever write their own cache files there.
#
# When sccache is on PATH it is used as the rustc wrapper, so a reinstall of
# unchanged crates is served from cache and only the changed ones recompile.
# Set KICADMIUM_SETUP_PLAIN=1 to skip all of this and build exactly as cargo
# would on its own.
set -eu
cd "$(dirname "$0")/.."

if [ -z "${KICADMIUM_SETUP_PLAIN:-}" ]; then
    # The account's real home, independent of the overridden HOME.
    real_home=$(eval echo "~$(id -un)" 2>/dev/null || true)
    if [ -n "$real_home" ] && [ -d "$real_home" ]; then
        if [ -z "${CARGO_HOME:-}" ] && [ -d "$real_home/.cargo" ]; then
            export CARGO_HOME="$real_home/.cargo"
        fi
        if [ -z "${RUSTUP_HOME:-}" ] && [ -d "$real_home/.rustup" ]; then
            export RUSTUP_HOME="$real_home/.rustup"
        fi
        # trunk (wasm-bindgen, wasm-opt) and sccache cache under XDG_CACHE_HOME.
        export XDG_CACHE_HOME="${XDG_CACHE_HOME_REAL:-$real_home/.cache}"
    fi
    if [ -z "${RUSTC_WRAPPER:-}" ] && command -v sccache >/dev/null 2>&1; then
        export RUSTC_WRAPPER=sccache
    fi
fi

echo "kicadmium setup: building release binary (CARGO_HOME=${CARGO_HOME:-default}, RUSTC_WRAPPER=${RUSTC_WRAPPER:-none})" >&2
cargo build --release -p backend
exec target/release/kicadmium setup "$@"
