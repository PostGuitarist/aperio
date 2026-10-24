# Architecture

This is the plan for v0.1. It records decisions that are expensive to change later; everything else can evolve.

## Goals that shape the design

- **Non-destructive.** Originals are read-only. Every edit is data in a sidecar ([format](sidecar-format.md)).
- **Local-first.** All processing on the user's machine. No account, no telemetry, and no network calls except downloading an AI model when the user asks for it.
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
  aperio-plugin/   plugin API and host (no renderer dependency)
plugins/
  vignette/        example third-party plugin
app/
  src/             TypeScript UI
  src-tauri/       Tauri backend: commands that call into the crates
docs/
```

`aperio-core` stays free of GPU and UI dependencies so it can be tested quickly and reused by a future CLI or batch exporter.

## How an edit flows

1. Open: decode the original into linear RGB with sRGB primaries (32-bit float) and load or create its sidecar. RAW files are demosaiced by rawler with the as-shot white balance and camera matrix applied.
2. Edit: the UI owns a copy of the `EditDocument`. Panels and on-canvas tools change it and send the whole document with each preview request; the backend keeps the latest one and saves the sidecar when an edit is committed (a slider released, a drag finished). Undo and redo are snapshots of the document in the UI.
3. Render: document operations compile into a flat list of per-pixel steps that a WGSL compute shader runs; a CPU copy of the same math is the fallback and the test reference. The renderer walks the layers bottom to top: one GPU dispatch per layer runs its steps on the result below and blends through the layer's mask (rasterized on the CPU at the image's size) at its opacity.
4. Preview: renders at screen resolution and is shared with the webview as an image. The preview can skip the crop (while the crop tool is open), tint the selected layer's mask red, or hide every adjustment for a before view; on-canvas handles are drawn in the UI over the image, mapped through the crop. Full resolution is used only for export and for 100% zoom tiles.
5. Export: full-resolution render, converted to the output color space (sRGB by default) and encoded.

## Plugins

A plugin declares an id, a version, and what it provides: operations, masks, or both. AI plugins also declare their model files and licenses. In v0.1 plugins are Rust crates built into the app behind a stable trait; loading plugins from outside the build (dynamic libraries or WASM) comes after the API has settled. The bundled sky and subject segmentation plugin is written against the same public API as third-party plugins. See [plugins.md](plugins.md).

The renderer runs layers without plugin operations on the GPU in one pass, as before. A layer with plugin operations runs on a copy of the image: its built-in steps (GPU) and plugin operations (CPU, or the plugin's own accelerator) run in document order, and the copy is blended back through the layer's mask. Plugin masks are generated from the decoded photo before adjustments and cached by the host.

## Open questions

- How previews reach the webview fastest (shared texture versus encoded frames) on each platform.
- Which open segmentation model gives the best quality for its license and size.
- Color management details: working space, and how camera profiles are sourced without using proprietary profiles.
