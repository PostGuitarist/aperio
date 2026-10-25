# Aperio

A free, open-source photo editor with AI-assisted tools that runs entirely on your own machine. No subscription, no account, no cloud, no proprietary formats.

> **Status:** v0.1, an early preview. Aperio opens RAW, JPEG, PNG, and TIFF files with a live GPU preview; edits exposure, contrast, highlights, shadows, whites, blacks, white balance, saturation, vibrance, and tone curves; crops and straightens; adds brush, gradient, and radial mask layers with their own adjustments; makes AI sky and subject masks on your own machine and replaces skies; saves every edit to a sidecar with undo and redo; and exports JPEG, PNG, or 16-bit TIFF. Installers for macOS, Windows, and Linux are on the [Releases](https://github.com/PostGuitarist/aperio/releases) page.

## Principles

- **Non-destructive.** Originals are never modified. Edits live in a readable JSON sidecar next to each photo ([format](docs/sidecar-format.md)).
- **Local-first.** Everything runs on your computer.
- **Open.** GPL-3.0-or-later, documented formats, and AI features as swappable plugins with audited licenses ([LICENSES.md](LICENSES.md)).

## What's in v0.1

See the [release notes](docs/release-notes.md). Architecture: [architecture.md](docs/architecture.md).

## Building

You need [Rust](https://rustup.rs) 1.90 or newer, Node.js 20 or newer, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform. On Linux, the prebuilt ONNX Runtime used for the AI masks needs glibc 2.38 or newer (Ubuntu 24.04, Debian 13, Fedora 39 or later).

```sh
cd app
npm install
npm run tauri dev     # run the app (or: npm run tauri dev -- -- photo.CR2)
npm run tauri build   # build an installer
```

Run the Rust tests from the repository root with `cargo test --workspace`.

Pushing a `v*` tag builds the installers on macOS, Windows, and Linux and attaches them to a draft GitHub release, using [docs/release-notes.md](docs/release-notes.md) as its description. Check the draft, then publish it.

To render a photo with its sidecar edits from the command line:

```sh
cargo run --release -p aperio-cli -- render photo.CR2 photo.jpg
cargo run --release -p aperio-cli -- plugins   # list bundled plugins
```

To write a plugin, see [docs/plugins.md](docs/plugins.md).

## License

[GPL-3.0-or-later](LICENSE).
