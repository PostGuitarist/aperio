# Aperio

A free, open-source photo editor with AI-assisted tools that runs entirely on your own machine. No subscription, no account, no cloud, no proprietary formats.

> **Status:** early development. Nothing here edits photos yet.

## Principles

- **Non-destructive.** Originals are never modified. Edits live in a readable JSON sidecar next to each photo ([format](docs/sidecar-format.md)).
- **Local-first.** Everything runs on your computer.
- **Open.** GPL-3.0-or-later, documented formats, and AI features as swappable plugins with audited licenses ([LICENSES.md](LICENSES.md)).

## v0.1 roadmap

1. Project skeleton, CI, and architecture ([architecture.md](docs/architecture.md))
2. Open RAW/JPEG, GPU preview, basic adjustments, export
3. Curves, crop, layers, and brush/gradient/radial masks
4. Plugin API, AI sky and subject segmentation, sky replacement

## Building

You need [Rust](https://rustup.rs) 1.90 or newer, Node.js 20 or newer, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform.

```sh
cd app
npm install
npm run tauri dev     # run the app
npm run tauri build   # build an installer
```

Run the Rust tests from the repository root with `cargo test --workspace`.

## License

[GPL-3.0-or-later](LICENSE).
