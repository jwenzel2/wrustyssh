# wrustyssh

A Slint-based SSH client for Windows with tabbed terminals, SFTP file transfers, and SSH key management.

## Prerequisites

### Rust Toolchain

Install Rust via [rustup](https://rustup.rs/):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Or on Windows, download and run the installer from https://rustup.rs.

The minimum supported Rust edition is **2021**. Any recent stable toolchain will work.

### C/C++ Compiler

A C/C++ compiler is required to build native dependencies.

- **Windows (recommended):** Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the "Desktop development with C++" workload. This provides MSVC, the Windows SDK, and all necessary headers/libraries.
- **Windows (alternative):** If using the `x86_64-pc-windows-gnu` target, install [MSYS2](https://www.msys2.org/) with the `mingw-w64-x86_64-toolchain` package.

### Additional System Dependencies

No additional system libraries need to be installed. All dependencies (SSH, terminal emulation, UI) are pure Rust crates pulled in via Cargo.

## Building

### Debug Build

```sh
cargo build
```

The binary is output to `target/debug/wrustyssh.exe`.

### Release Build

```sh
cargo build --release
```

The binary is output to `target/release/wrustyssh.exe`. Release builds enable optimizations and are significantly faster at runtime.

## Running

```sh
cargo run
```

Or for a release build:

```sh
cargo run --release
```

You can also run the compiled binary directly:

```sh
./target/release/wrustyssh.exe
```

### Environment Variables

- `RUST_LOG` — Controls log verbosity (uses `env_logger`). Examples:
  - `RUST_LOG=info cargo run` — Show info-level logs
  - `RUST_LOG=debug cargo run` — Show debug-level logs
  - `RUST_LOG=wrustyssh=debug cargo run` — Debug logs for this crate only

## Creating an Installer (Optional)

The project includes WiX metadata in `Cargo.toml` for building an MSI installer with [cargo-wix](https://github.com/volks73/cargo-wix):

```sh
cargo install cargo-wix
cargo wix
```

This requires [WiX Toolset v3](https://wixtoolset.org/) to be installed on your system.

## Project Structure

```
wrustyssh/
├── src/
│   ├── main.rs          # Entry point
│   ├── app.rs           # Application logic
│   ├── config.rs        # Configuration management
│   ├── error.rs         # Error types
│   ├── keys/            # SSH key management
│   ├── models/          # Data models (connections, etc.)
│   ├── ssh/             # SSH session and SFTP implementation
│   ├── storage/         # Persistent storage
│   └── ui/              # Slint UI definitions and handlers
│       └── window.slint # Main UI layout
├── build.rs             # Build script (compiles Slint UI and embeds Windows resources)
├── app.rc               # Windows resource file (application icon)
├── wrusty.ico           # Application icon
└── Cargo.toml           # Project manifest and dependencies
```

## Key Dependencies

| Crate | Purpose |
|-------|---------|
| `slint` | GUI framework |
| `russh` / `russh-keys` / `russh-sftp` | SSH protocol, key handling, and SFTP |
| `ssh-key` | SSH key parsing and generation |
| `vt100` | Terminal emulation |
| `tokio` | Async runtime |
| `rfd` | Native file dialogs |
| `arboard` | Clipboard access |
| `fontdue` | Font rendering for the terminal |
| `serde` / `serde_json` | Configuration serialization |
