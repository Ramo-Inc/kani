<p align="center">
  <img src="crates/kani-gui/assets/app_128x128.png" alt="Kani logo" width="128" />
</p>

<h1 align="center">Kani</h1>

<p align="center">
  <strong>Share your keyboard, mouse, and clipboard across machines — seamlessly.</strong>
</p>

<p align="center">
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-1.78%2B-orange?logo=rust" alt="Rust"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="MIT License"></a>
  <a href="#"><img src="https://img.shields.io/badge/Platform-macOS%20%7C%20Windows-lightgrey" alt="Platform"></a>
  <a href="#"><img src="https://img.shields.io/badge/Status-Alpha-yellow" alt="Status"></a>
</p>

<p align="center">
  <a href="README.ja.md">日本語</a>
</p>

---

Kani is a free, open-source software KVM that lets you control multiple computers with a single keyboard and mouse over your local network. Built in Rust with UDP for low-latency input and DTLS encryption for security.

---

## Features

- **Cross-platform** — macOS and Windows, first-class support for both
- **Low latency** — UDP-based input forwarding, no TCP head-of-line blocking
- **Encrypted** — DTLS (Datagram TLS) secures all input traffic over the network
- **Clipboard sync** — Copy on one machine, paste on another (text)
- **Visual layout editor** — Drag-and-drop display arrangement in the GUI
- **System tray** — Runs quietly in the background, close-to-tray
- **Host/Client mode** — One machine hosts, others connect automatically
- **Modifier remapping** — Ctrl↔Cmd automatically remapped between macOS and Windows
- **Emergency hotkey** — Ctrl+Alt+Esc always returns cursor to your local machine

## How It Works

Move your mouse to the edge of your screen — the cursor seamlessly jumps to the next machine. Keyboard input follows the cursor. Clipboard contents sync automatically.

```
  ┌──────────────┐         UDP/DTLS          ┌──────────────┐
  │    macOS     │◄─────────────────────────►│   Windows    │
  │  (Host)      │   keyboard + mouse + clip  │  (Client)    │
  └──────────────┘                            └──────────────┘
```

## Screenshots

<p align="center">
  <img src="assets/display_layout.png" alt="Display Layout — drag-and-drop multi-monitor arrangement" width="720" />
</p>

<p align="center">
  <em>Visual display layout editor — drag monitors to match your physical desk setup. Supports multi-display per machine.</em>
</p>

<br>

<p align="center">
  <img src="assets/Settings.png" alt="Settings — Host/Client configuration" width="480" />
</p>

<p align="center">
  <em>Settings panel — configure Host/Client role, manage remote machines, one-click KVM start.</em>
</p>

## Why Kani?

| | **Kani** | Synergy | Deskflow | Lan Mouse |
|---|---|---|---|---|
| **Price** | Free | $29+ | Free | Free |
| **License** | MIT | Proprietary | GPL | GPL |
| **Language** | Rust | C++ | C++ | Rust |
| **Protocol** | UDP/DTLS | TCP/TLS | TCP/TLS | UDP/DTLS |
| **Clipboard sync** | Yes | Yes | Yes | No |
| **GUI config** | Yes | Yes | Yes | Minimal |
| **macOS + Windows** | First-class | Yes | Yes | Secondary |
| **Latency** | Low (UDP) | Higher (TCP) | Higher (TCP) | Low (UDP) |

**Kani combines the best of both worlds:** Rust + UDP performance from Lan Mouse, with clipboard sync and a full GUI like Deskflow — under the commercially-friendly MIT license.

## Installation

### Pre-built Binaries

> Coming soon. For now, build from source.

### Build from Source

**Requirements:**
- Rust 1.78+ ([rustup.rs](https://rustup.rs/))
- macOS 12+ or Windows 10/11
- Windows: Visual Studio Build Tools (C++ workload)

```bash
git clone https://github.com/Ramo-Inc/kani.git
cd kani
cargo build --workspace --release
```

The binaries will be in `target/release/`.

## Quick Start

### 1. Launch the GUI on both machines

```bash
cargo run -p kani-gui --release
```

### 2. Configure

1. Go to the **Settings** tab and note your **Host ID**
2. Add the remote machine (Host ID, IP address, display resolution)
3. Switch to **Display Layout** and drag displays next to each other
4. Click **Save Configuration**
5. Repeat on the other machine

### 3. Choose a role

- **Host machine**: Select "Host" role in Settings — starts the KVM server
- **Client machine**: Select "Client" role, enter the Host's IP — connects automatically

### 4. Start KVM

Click **Start KVM** in the GUI. Move your mouse to the screen edge — it jumps to the other machine.

**Emergency return:** Press **Ctrl+Alt+Esc** at any time to force the cursor back to your local machine.

### Firewall

Allow UDP port **24900** on both machines:

```powershell
# Windows (Admin PowerShell)
New-NetFirewallRule -DisplayName "Kani KVM" -Direction Inbound -Protocol UDP -LocalPort 24900 -Action Allow
```

macOS allows inbound UDP by default. If blocked:
```bash
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --add /path/to/kani-gui
```

### macOS Permissions

Kani requires two macOS privacy permissions:

1. **Input Monitoring** — System Settings > Privacy & Security > Input Monitoring
2. **Accessibility** — System Settings > Privacy & Security > Accessibility

Add your terminal or the `kani-gui` binary to both.

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Language | Rust |
| Async runtime | tokio |
| Input protocol | UDP + bincode (512-byte packets) |
| Encryption | DTLS (webrtc-dtls) |
| Clipboard transport | TCP with length-framed messages |
| GUI | egui / eframe |
| System tray | tray-icon / muda |
| macOS input | CGEventTap |
| Windows input | Raw Input API + SendInput |

## Architecture

```
kani/
├── crates/
│   ├── kani-proto/       # Wire protocol, config, topology, shared types
│   ├── kani-core/        # Virtual cursor, edge detection, state machine
│   ├── kani-platform/    # OS abstraction (macOS CGEventTap / Windows Raw Input)
│   ├── kani-transport/   # UDP, DTLS, connection management, TCP clipboard
│   ├── kani-clipboard/   # Clipboard monitoring & sync
│   ├── kani-app/         # CLI binary & KvmEngine
│   └── kani-gui/         # GUI with system tray (egui + tray-icon)
└── examples/             # Example config files
```

## Roadmap

- [x] Core KVM (mouse + keyboard forwarding)
- [x] DTLS encryption
- [x] Clipboard sync (text)
- [x] GUI display layout editor
- [x] Host/Client architecture
- [x] System tray with close-to-tray
- [x] Modifier remapping (Ctrl↔Cmd)
- [ ] Auto-discovery (mDNS)
- [ ] Image clipboard sync
- [ ] File transfer
- [ ] Linux / Wayland support
- [ ] Pre-built release binaries

## Development

```bash
# Build
cargo build --workspace

# Test
cargo test --workspace

# Lint (CI enforces these)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Dry-run mode (no platform input)
cargo run -p kani-app -- --config examples/kani-example.toml --dry-run
```

## Contributing

Contributions are welcome! Whether it's bug reports, feature requests, or pull requests.

```bash
# Setup
git clone https://github.com/Ramo-Inc/kani.git
cd kani
cargo build --workspace
cargo test --workspace
```

Please run `cargo fmt` and `cargo clippy` before submitting PRs.

## License

[MIT](LICENSE)

## Author

**Ramo** — [@Ramo-Inc](https://github.com/Ramo-Inc)
