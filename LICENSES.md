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
| thiserror | core, render | MIT OR Apache-2.0 |
| tauri | app | MIT OR Apache-2.0 |
| tauri-build | app (build) | MIT OR Apache-2.0 |
| tauri-plugin-dialog | app (open/save dialogs) | MIT OR Apache-2.0 |
| rawler | render (RAW decoding) | LGPL-2.1-only. Compatible: LGPL-2.1 §3 allows using the code under GPL-2.0-or-later. Declared as the deprecated id `LGPL-2.1`, clarified in deny.toml. |
| image | render (JPEG/PNG/TIFF), sky plugin, app (library thumbnails) | MIT OR Apache-2.0 |
| kamadak-exif | render (reading EXIF, writing it into exports) | BSD-2-Clause. Pulls in mutate_once (BSD-2-Clause). |
| tiff | render (16-bit TIFF export with metadata tags) | MIT |
| wgpu | render (GPU) | MIT OR Apache-2.0 |
| bytemuck | render | Zlib OR Apache-2.0 OR MIT |
| pollster | render | Apache-2.0 OR MIT |
| log | render | MIT OR Apache-2.0 |
| ort | segment plugin (runs ONNX models) | MIT OR Apache-2.0. Links ONNX Runtime (MIT), downloaded prebuilt at build time and linked statically. On Windows it also links DirectML; see below. |
| ureq | segment plugin (model downloads) | MIT OR Apache-2.0 |
| sha2 | segment plugin (model checksums) | MIT OR Apache-2.0 |
| dirs | segment plugin (models folder), app (thumbnail cache) | MIT OR Apache-2.0 |

On Windows, ONNX Runtime uses DirectML (Microsoft, proprietary but redistributable under the [Microsoft Software License Terms](https://www.nuget.org/packages/Microsoft.AI.DirectML/1.15.4/License)) to run models on the GPU. Windows 10 1903 and later include DirectML as a system component. Development builds copy `DirectML.dll` next to the executable. Whether installers ship it, or rely on the system copy, has to be settled before the first Windows release.

Workspace crates are GPL-3.0-or-later, except the example plugin `aperio-plugin-vignette`, which is MIT OR Apache-2.0 so it can be copied freely as a template for third-party plugins.

Transitive Rust dependencies (several hundred crates, mostly Tauri's platform layers) are all under licenses from the allow-list. Run `cargo deny list -l license` for the full current list.

## JavaScript packages (direct dependencies)

| Package | Used for | License |
|---|---|---|
| @tauri-apps/api | frontend ↔ backend calls | MIT OR Apache-2.0 |
| @tauri-apps/cli | build tooling only | MIT OR Apache-2.0 |
| @tauri-apps/plugin-dialog | open/save dialogs | MIT OR Apache-2.0 |
| typescript | build tooling only | Apache-2.0 |
| vite | build tooling only | MIT |

## AI models

Models are not bundled with the app or the repository. They're downloaded once, when the user asks (in the app, or with `aperio models download`), from the pinned revision below, and checked against a SHA-256 before use. See `plugins/segment/src/models.rs`.

| Model | Purpose | Weights license | Training data | Distribution |
|---|---|---|---|---|
| [Sky segmentation (U²-Net)](https://github.com/xiongzhu666/Sky-Segmentation-and-Post-processing) by xiongzhu666 | AI sky mask | MIT (repository LICENSE; the ONNX mirror at [JianyuanWang/skyseg](https://huggingface.co/JianyuanWang/skyseg) restates it) | Not published. Architecture from [U²-Net](https://github.com/xuebinqin/U-2-Net) (Apache-2.0). | Optional download, 176 MB, `skyseg.onnx` at revision `3ba8c6d` |
| [BiRefNet lite](https://github.com/ZhengPeng7/BiRefNet) by Peng Zheng et al. | AI subject mask | MIT ([model card](https://huggingface.co/ZhengPeng7/BiRefNet_lite)) | Public academic segmentation datasets listed in the BiRefNet README (e.g. DIS5K) | Optional download, 224 MB, ONNX export at [onnx-community/BiRefNet_lite-ONNX](https://huggingface.co/onnx-community/BiRefNet_lite-ONNX) revision `de15b22` |
| [ISNet general use (DIS)](https://github.com/xuebinqin/DIS) by Xuebin Qin et al. | Faster AI subject mask. **Not offered by default builds**: only with the `fast-subject` Cargo feature, which is off. | None stated. The DIS code is Apache-2.0, but the weights' release doesn't state a license. | [DIS5K](https://github.com/xuebinqin/DIS), whose terms allow non-commercial research and education only | With the feature on, a separate download the user chooses after a notice about these terms (never part of "Download models"), 179 MB, rembg's ONNX export from its [`v0.0.0` release](https://github.com/danielgatis/rembg/releases/tag/v0.0.0), SHA-256 `60920e99…` |

## Fonts, icons, and other assets

| Asset | License |
|---|---|
| App icon (`app/src-tauri/icons/`) | Original work, GPL-3.0-or-later |
