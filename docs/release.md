# Release artifacts

The terminal client builds as one native binary and does not require another
coding-agent CLI at runtime. The Tauri desktop client lives in `desktop/` but is
not yet part of the public release artifacts described here.

## Local release build

```bash
cargo build --release
```

The checked-in release profile uses thin LTO, one codegen unit, symbol
stripping, and abort-on-panic. The release binary is:

```text
target/release/mjolnr
```

## Supported artifact targets

The CI workflow builds these targets when the hosted runner is available:

- `aarch64-apple-darwin` — macOS arm64
- `x86_64-apple-darwin` — macOS x64
- `x86_64-unknown-linux-gnu` — Linux x64
- `aarch64-unknown-linux-gnu` — Linux arm64

Each artifact is named `mjolnr-<target>` and is accompanied by a SHA-256 file.
Verify one with:

```bash
shasum -a 256 -c mjolnr-<target>.sha256
```

## Install

Copy the native binary to a directory on `PATH`, for example:

```bash
install -m 0755 mjolnr-<target> "$HOME/.local/bin/mjolnr"
mjolnr --version
```

The project does not currently claim Windows support, release signing,
notarization, or OS-level containment. Each claim requires its own implemented
and verified release gate.
