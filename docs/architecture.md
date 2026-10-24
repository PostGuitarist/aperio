# Architecture

This is the plan for v0.1. It records decisions that are expensive to change later; everything else can evolve.

## Goals that shape the design

- **Non-destructive.** Originals are read-only. Every edit is data in a sidecar ([format](sidecar-format.md)).
- **Local-first.** All processing on the user's machine. No account, no network calls, no telemetry.
- **Cross-platform.** Windows, macOS, and Linux from one codebase. CI checks Linux on every PR; macOS and Windows builds run for release tags or on demand to save Actions minutes.
- **Modular AI.** AI features are plugins with swappable models.

## Stack

| Concern | Choice | Why |
|---|---|---|
| Core language | Rust | Fast, memory-safe image code; one codebase for all platforms. |
| App shell | Tauri 2 | Small native binaries on all three platforms; UI iterates quickly in HTML/TypeScript. |
| UI | TypeScript + Vite, no framework yet | Keep it light until the mockups settle; add a framework only if the UI needs it. |
| GPU processing | wgpu (WGSL shaders) | One API over Metal, Vulkan, and DirectX 12. |
| RAW decoding | rawler (LGPL-2.1) | Pure Rust, no C toolchain needed. LibRaw stays an option if users hit unsupported cameras. |
| Other formats | `image` crate (JPEG, PNG, TIFF); libheif for HEIC later | |
| AI inference | ONNX Runtime via the `ort` crate | Runs open models on CPU, CoreML, DirectML, or CUDA. |

## Layout

```
crates/
  aperio-core/     edit document, operations, masks, sidecar I/O (no GPU, no UI)
  aperio-render/   decode, wgpu adjustment pipeline (with a CPU fallback), export
  aperio-cli/      `aperio render <in> <out>`: headless render using the sidecar
  aperio-plugin/   (milestone 4) plugin API and host
app/
  src/             TypeScript UI
  src-tauri/       Tauri backend: commands that call into the crates
docs/
```

`aperio-core` stays free of GPU and UI dependencies so it can be tested quickly and reused by a future CLI or batch exporter.

## How an edit flows

1. Open: decode the original into linear RGB with sRGB primaries (32-bit float) and load or create its sidecar. RAW files are demosaiced by rawler with the as-shot white balance and camera matrix applied.
2. Edit: the UI sends a change to the backend, which updates the `EditDocument` and saves the sidecar.
3. Render: document operations compile into a flat list of per-pixel steps that a WGSL compute shader runs; a CPU copy of the same math is the fallback and the test reference. The renderer will walk the layers bottom to top. Each layer's operations run as GPU passes on the layer below; the result is blended through the layer's mask at its opacity.
4. Preview: renders at screen resolution and is shared with the webview as an image. Full resolution is used only for export and for 100% zoom tiles.
5. Export: full-resolution render, converted to the output color space (sRGB by default) and encoded.

## Plugins (milestone 4)

A plugin declares an id, a version, and what it provides: operations, masks, or both. AI plugins also declare their model files and licenses. In v0.1 plugins are Rust crates built into the app behind a stable trait; loading plugins from outside the build (dynamic libraries or WASM) comes after the API has settled. The bundled sky and subject segmentation plugin is written against the same public API as third-party plugins.

## Open questions

- How previews reach the webview fastest (shared texture versus encoded frames) on each platform.
- Which open segmentation model gives the best quality for its license and size.
- Color management details: working space, and how camera profiles are sourced without using proprietary profiles.
