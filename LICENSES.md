# Licenses

Aperio is licensed under the **GNU General Public License v3.0 or later** (see [LICENSE](LICENSE)).

This file records every direct dependency and every AI model Aperio ships or downloads, with its license and why it is compatible with GPL-3.0-or-later. CI enforces the Rust side with `cargo deny check licenses` using the allow-list in [deny.toml](deny.toml); a new license must be added there and here in the same PR.

## Rules

- Only licenses compatible with GPL-3.0-or-later are allowed: permissive (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode, CC0, Unlicense), weak copyleft (MPL-2.0, LGPL), or GPL-3.0 itself.
- No code, presets, assets, or model weights from commercial photo editors. Every feature is built from public research, open models, and our own work.
- Model weights need their own entry below, including the license of the training data where it is published. Weights under non-commercial or research-only terms are not bundled; at most they are offered as an optional download the user chooses.

## Rust crates (direct dependencies)

| Crate | Used by | License |
|---|---|---|
| serde | core, app | MIT OR Apache-2.0 |
| serde_json | core, app | MIT OR Apache-2.0 |
| thiserror | core | MIT OR Apache-2.0 |
| tauri | app | MIT OR Apache-2.0 |
| tauri-build | app (build) | MIT OR Apache-2.0 |

Transitive Rust dependencies (about 380 crates, mostly Tauri's platform layers) are all under licenses from the allow-list. Run `cargo deny list -l license` for the full current list.

## JavaScript packages (direct dependencies)

| Package | Used for | License |
|---|---|---|
| @tauri-apps/api | frontend ↔ backend calls | MIT OR Apache-2.0 |
| @tauri-apps/cli | build tooling only | MIT OR Apache-2.0 |
| typescript | build tooling only | Apache-2.0 |
| vite | build tooling only | MIT |

## AI models

None yet. The first entry will be the segmentation model chosen in the AI plugin milestone.

| Model | Purpose | Weights license | Training data | Distribution |
|---|---|---|---|---|

## Fonts, icons, and other assets

| Asset | License |
|---|---|
| App icon (`app/src-tauri/icons/`) | Original work, GPL-3.0-or-later |
