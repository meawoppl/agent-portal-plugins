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
# Serialize activation and expose an in-progress marker to the running binary.
mkdir .install-lock || { echo "kicadmium install already running; inspect .install-lock/pid" >&2; exit 1; }
trap 'rm -f .install-lock/pid; rmdir .install-lock' EXIT
trap 'exit 130' INT TERM
printf '%s\n' "$$" > .install-lock/pid
# Bootstrap the stable runtime on upgrade before touching any build output.
if [ ! -x bin/kicadmium ] && [ -x target/release/kicadmium ]; then
    mkdir -p bin
    cp -p target/release/kicadmium "bin/.bootstrap.$$"
    mv "bin/.bootstrap.$$" bin/kicadmium
fi
export CARGO_TARGET_DIR="${KICADMIUM_SETUP_TARGET_DIR:-$PWD/target/install-build}"
cargo build --release -p backend
"$CARGO_TARGET_DIR/release/kicadmium" setup --activate "$PWD/bin/kicadmium" "$@"
