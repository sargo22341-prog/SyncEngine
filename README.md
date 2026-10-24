<p align="center">
  <strong>English</strong> ·
  <a href="README.fr.md">Français</a>
</p>

<p align="center">
  <img src="./docs/icon/syncengine.png" width="120" alt="SyncEngine logo" />
</p>

<h1 align="center">SyncEngine</h1>

<p align="center">
  A native backup, mirror and two-way sync tool for Windows and Linux:<br />
  simulate first, then copy — and never lose a file to a guess.
</p>

<p align="center">
  <a href="https://github.com/sargo22341-prog/SyncEngine/releases/latest"><img src="./docs/images/badge_github.png" height="80" alt="Get it on GitHub" /></a>
</p>

## Disclaimer

> [!WARNING]
> This application was **developed with the help of artificial intelligence**.
> The code, tests and documentation were largely produced by AI and then checked by automated
> tests, but not everything has been reviewed line by line or validated in every real-world
> situation. SyncEngine writes, replaces and can delete files: try it on copies first, keep
> independent backups, and see the [known limitations](#known-limitations).

## Overview

**SyncEngine** keeps folders in step with one another, in the spirit of SyncBack or
FreeFileSync, with a strong bias towards data safety: every run starts with a **simulation**
that shows exactly what will be copied, replaced or deleted, and nothing is written until you
confirm that precise plan.

It is written entirely in Rust: a native desktop interface ([iced](https://iced.rs), no web
view), a command-line tool and a headless agent, all sharing the same engines.

Main features:

- **three modes**, each with its own rules:
  - **backup**: source → destination, never deletes anything;
  - **mirror**: makes the destination identical to the source, deletions only with an explicit
    policy, a separate confirmation and a cap on mass deletions;
  - **two-way sync** between two local folders, with a reliable previous state, tombstones for
    deletions and conflicts kept side by side instead of overwritten;
- **simulation before writing**: the plan shown is the plan executed, with a count per action
  (copy, replace, delete, conflict, ignore, keep), a reason for every path, filters and search;
- **safety guards**: a missing drive, an unreadable folder or a partial scan is never treated as
  "empty" and never leads to a deletion; a destination nested in its source is refused;
  preconditions are checked again right before each write;
- **verified copies**: files are written to a temporary file then checked (BLAKE3 content or
  size), optional versioning of replaced and deleted files, a journal that survives a crash;
- **fast comparison**: by size and date (only directory listings are read), or by content;
- **profiles and groups**: a seven-step wizard (no JSON), groups with drag and drop, profiles
  with or without a group, filters (include, exclude, excluded folders);
- **live progress** (operations, bytes, throughput), cancellation at any time with the partial
  state kept and logged, and a **history** of every run;
- **scheduling** through a headless agent that keeps running when the window is closed, with
  explicit authorization per profile;
- **experimental network connectors**: SFTP, WebDAV over HTTPS, FTP, FTPS and SMB, plus a
  **read-only OneDrive source**; they are labelled as experimental in the interface;
- dark and light themes following the system, keyboard shortcuts, notification-area icon;
- **no account, no telemetry, no cloud service** required.

### Preview

| Overview | Simulation | Copy in progress |
| --- | --- | --- |
| ![Overview of the profiles, grouped and ungrouped](docs/images/overview.png) | ![Simulation of a mirror with one deletion](docs/images/simulation.png) | ![Real copy in progress with live throughput](docs/images/execution.png) |

The screenshots use made-up folders, profiles and files. The interface is in French for now.

## Installation

Each version is published on the repository's GitHub releases as a **portable archive**: unzip
it in a folder of your own and start `syncengine-desktop`. The three executables (interface,
command line, agent) must stay side by side.

| System | File |
| --- | --- |
| Windows 11 x86_64 | `SyncEngine_X.Y.Z_x86_64-pc-windows-msvc.zip` |
| Linux x86_64 (built on Ubuntu 24.04) | `SyncEngine_X.Y.Z_x86_64-unknown-linux-gnu.tar.gz` |

Each archive holds only the three executables, `LICENSE.txt` and `THIRD_PARTY_NOTICES.txt`.
Check the download against the SHA-256 checksums listed at the end of the release description
(`sha256sum FILE` on Linux, `Get-FileHash -Algorithm SHA256 FILE` in PowerShell). The binaries are **not signed**: Windows
SmartScreen may warn on first launch.

No administrator rights are needed. Profiles, history and settings live in
`%APPDATA%\org.syncengine.desktop` (Windows) or `~/.local/share/org.syncengine.desktop` (Linux);
removing the program never deletes them.

### Building from source

Requirements: the Rust toolchain pinned in `rust-toolchain.toml` (installed automatically by
`rustup`) and, on Debian/Ubuntu, `build-essential pkg-config libssl-dev zlib1g-dev`.
Details: [docs/DEVELOPPEMENT.md](docs/DEVELOPPEMENT.md).

```sh
cargo run -p syncengine-desktop                                       # local folders
cargo build --release -p syncengine-desktop -p syncengine-cli -p syncengine-agent --features network,onedrive
```

## Command line

The same engines are available without the interface:

```sh
syncengine preview ./source ./destination                  # plan, nothing is written
syncengine preview ./source ./destination --mode mirror --allow-delete --max-delete 100
syncengine execute ./source ./destination --confirm --journal ./run-01.jsonl --progress
syncengine journal ./run-01.jsonl                          # diagnose a run
```

Every command, option and exit code: [docs/CLI.md](docs/CLI.md).

## Documentation

In French:

- [User guide](docs/GUIDE.md): modes, simulation, safeguards, interface, data folder;
- [Command line](docs/CLI.md);
- [Scheduling and agent](docs/PLANIFICATION.md);
- [Network and cloud connectors](docs/CONNECTEURS.md);
- [Architecture](docs/ARCHITECTURE.md);
- [Development](docs/DEVELOPPEMENT.md): build, checks, releases.

## Known limitations

- The interface is only available in **French**.
- Binaries are **not signed** and there is no installer yet: portable archives only.
- Network connectors and the OneDrive source are **experimental**; they were tested against
  isolated test servers, not against real providers or a real Microsoft account. SMB is
  read-only on Linux. Two-way sync is limited to two local folders.
- Remote servers do not offer global transactions: an interrupted run against a server can
  leave a partial state (always logged). Avoid other programs writing to the same folders
  during a run.
- Scheduled runs need native tasks: one click on Windows, manual systemd setup on Linux; see
  [scheduling](docs/PLANIFICATION.md).
- Linux builds are checked on Ubuntu 24.04 in CI; Debian is not compiled natively.
- `cargo audit` reports warnings (unmaintained or unsound crates) inherited from the graphical
  toolkit and the Linux tray icon; no known vulnerability. Any new warning fails CI; see the
  [tolerated advisories](docs/DEVELOPPEMENT.md#avis-rustsec-tolérés) (French).

## Stack

| Layer | Technology |
| --- | --- |
| Language | Rust (edition 2024), workspace of single-purpose crates |
| Interface | iced 0.14 (wgpu, tiny-skia fallback), rfd, tray-icon |
| Hashing | BLAKE3 |
| State | SQLite (rusqlite, versioned migrations) |
| Network | ssh2 (SFTP), reqwest + native-tls (WebDAV, OneDrive), SuppaFTP (FTP/FTPS) |
| Platforms | Windows x86_64 (MSVC), Linux x86_64 (GNU) |

## Contributing

Contribution rules (humans and AI agents): [`AGENTS.md`](AGENTS.md). Every change to the
application adds a short line to [`RELEASE_NOTES.md`](RELEASE_NOTES.md); the CI turns those lines
into the next GitHub release. Build, checks and release process:
[docs/DEVELOPPEMENT.md](docs/DEVELOPPEMENT.md).

## License

Copyright © 2026 sargo.

SyncEngine is free software, distributed under the **GNU General Public License, version 3 only**
(GPL-3.0-only): you may use, study, modify and redistribute it, provided that what you distribute
stays under the same license, with its source code. It comes with no warranty. Full text:
[`LICENSE`](LICENSE). Dependencies keep their own licenses; each archive ships their notices
(`THIRD_PARTY_NOTICES.txt`).

SyncEngine is an independent project, not affiliated with SyncBack, FreeFileSync, Microsoft or
any storage provider.
