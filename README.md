# wrustyssh

A Windows SSH client built with Rust and [Slint](https://slint.dev/), featuring tabbed terminals, SFTP file transfers, SSH key management, port forwarding, and Cloudflare Tunnel support.

> This is a Windows port of [grustyssh](https://github.com/jwenzel2/grustysshwin), originally a GTK4/libadwaita Linux application. The UI has been rebuilt with Slint for native Windows support.

## Prerequisites

### Rust Toolchain

Download and run the Rust installer from https://rustup.rs.

The minimum supported Rust edition is **2021**. Any recent stable toolchain will work. The project targets `x86_64-pc-windows-msvc` by default (configured in `.cargo/config.toml`).

### Visual Studio Build Tools

Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the **"Desktop development with C++"** workload. This provides the MSVC compiler, Windows SDK, and linker required by Rust.

## Building

### Debug Build

```sh
cargo build
```

The binary is output to `target/x86_64-pc-windows-msvc/debug/wrustyssh.exe`.

### Release Build

```sh
cargo build --release
```

The binary is output to `target/x86_64-pc-windows-msvc/release/wrustyssh.exe`. Release builds enable optimizations and are significantly faster at runtime.

## Running

```sh
cargo run
```

Or for a release build:

```sh
cargo run --release
```

### Logging

Set the `RUST_LOG` environment variable to control log verbosity:

```sh
set RUST_LOG=info && cargo run
set RUST_LOG=debug && cargo run
set RUST_LOG=wrustyssh=debug && cargo run
```

## Creating an MSI Installer (Optional)

The project includes WiX metadata in `Cargo.toml` and a WiX template in `wix/` for building an MSI installer with [cargo-wix](https://github.com/volks73/cargo-wix):

```sh
cargo install cargo-wix
cargo wix
```

This requires [WiX Toolset v3](https://wixtoolset.org/) to be installed.

## Project Structure

```
wrustyssh/
├── .cargo/
│   └── config.toml      # Build target configuration (MSVC)
├── src/
│   ├── main.rs           # Entry point
│   ├── app.rs            # Application logic
│   ├── config.rs         # Configuration management
│   ├── error.rs          # Error types
│   ├── keys/             # SSH key management
│   ├── models/           # Data models (connections, etc.)
│   ├── ssh/              # SSH session and SFTP implementation
│   ├── storage/          # Persistent storage
│   └── ui/               # Slint UI definitions and handlers
│       └── window.slint  # Main UI layout
├── fonts/                # Bundled fonts for terminal rendering
├── wix/                  # WiX installer template
├── build.rs              # Build script (compiles Slint UI, embeds Windows resources)
├── app.rc                # Windows resource file (embeds application icon)
├── wrusty.ico            # Application icon
└── Cargo.toml            # Project manifest and dependencies
```

## Key Dependencies

| Crate | Purpose |
|-------|---------|
| `slint` | GUI framework |
| `russh` / `russh-keys` / `russh-sftp` | SSH protocol, key handling, and SFTP |
| `ssh-key` | SSH key parsing and generation |
| `vt100` | Terminal emulation |
| `fontdue` | Font rendering for the terminal |
| `tokio` | Async runtime |
| `rfd` | Native file dialogs |
| `arboard` | Clipboard access |
| `serde` / `serde_json` | Configuration serialization |
