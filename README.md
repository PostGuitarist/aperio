# Aperio

A free, open-source photo editor with AI-assisted tools that runs entirely on your own machine. No subscription, no account, no cloud, no proprietary formats.

> **Status:** v0.2, an early preview.

![Aperio's editor with a replaced sky](docs/images/editor-sky.jpg)

## What it does

- **Opens** RAW files from most cameras, JPEG, PNG, TIFF, and HEIC, with a live GPU preview.
- **Library** that reads your folders in place: no import, no catalog.
- **Adjusts** exposure, contrast, highlights, shadows, whites, blacks, white balance, vibrance, saturation, and tone curves; crops and straightens.
- **Layers and masks:** brush, gradient, and radial masks, each layer with its own adjustments.
- **AI masks for sky, subject, and background**, computed on your machine by open models, and **sky replacement** that relights the scene to match.
- **AI denoise** for high-ISO photos, run locally on the full-size image.
- **Exports** JPEG, PNG, or 16-bit TIFF, with resizing, output sharpening, and metadata you choose.
- **Plugins:** the AI tools are plugins, and you can write your own ([docs/plugins.md](docs/plugins.md)).

**New here?** Download an installer from [Releases](https://github.com/PostGuitarist/aperio/releases), then follow [Getting started](docs/getting-started.md). The [release notes](docs/release-notes.md) list what's in v0.2 and its known limitations.

## Principles

- **Non-destructive.** Originals are never modified. Edits live in a readable JSON sidecar next to each photo ([format](docs/sidecar-format.md)).
- **Local-first.** Everything runs on your computer. No account, no telemetry.
- **Open.** GPL-3.0-or-later, documented formats, and AI features as swappable plugins with audited licenses ([LICENSES.md](LICENSES.md)).

Architecture: [docs/architecture.md](docs/architecture.md). What's next: [docs/ROADMAP.md](docs/ROADMAP.md).

## Building

You need [Rust](https://rustup.rs) 1.90 or newer, Node.js 20 or newer, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform. On Linux, the prebuilt ONNX Runtime used for the AI masks needs glibc 2.38 or newer (Ubuntu 24.04, Debian 13, Fedora 39 or later).

```sh
cd app
npm install
npm run tauri dev     # run the app (or: npm run tauri dev -- -- photo.CR2)
npm run tauri build   # build an installer
```

Run the Rust tests from the repository root with `cargo test --workspace`.

To release, open **Actions → Release → Run workflow** and enter a tag such as `v0.1.0` (it is created on `main` if it doesn't exist), or push a `v*` tag. The workflow builds the installers on macOS, Windows, and Linux and publishes a GitHub release with them attached, titled "Aperio <tag>", using [docs/release-notes.md](docs/release-notes.md) followed by the merged pull requests since the last release as its description. Tags with a suffix, like `v0.1.0-rc1`, become pre-releases. The tag must match the version in `Cargo.toml`, `app/package.json`, and `app/src-tauri/tauri.conf.json`.

To render a photo with its sidecar edits from the command line:

```sh
cargo run --release -p aperio-cli -- render photo.CR2 photo.jpg
cargo run --release -p aperio-cli -- plugins   # list bundled plugins
```

To write a plugin, see [docs/plugins.md](docs/plugins.md).

## Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org). The macOS app is signed with the maintainer's Apple Developer ID.

Only installers built by this repository's [Release workflow](.github/workflows/release.yml) on GitHub-hosted runners, from source in this repository, are signed. How signing is set up is in [docs/signing.md](docs/signing.md).

- Committers and reviewers: [@PostGuitarist](https://github.com/PostGuitarist)
- Approvers: [@PostGuitarist](https://github.com/PostGuitarist)

Every change from someone outside the team is reviewed before it's merged, and every signing request is approved by hand.

## Privacy

This program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it. The only network access is downloading the AI models (from GitHub and Hugging Face), when you ask Aperio to.

## License

[GPL-3.0-or-later](LICENSE).
