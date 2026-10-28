# Roadmap

This is the plan after v0.2.0. It lists what comes next and in what order, not when: Aperio is built by one maintainer, and each release ships when it's ready. The order can change as people use the app and report what they need most. Suggestions and bug reports are welcome on [Issues](https://github.com/PostGuitarist/aperio/issues).

## Where things stand

v0.1 covered the core workflow end to end: open RAW, JPEG, PNG, TIFF, and HEIC; adjust tone, color, and curves; crop; build layers with brush, gradient, radial, and AI sky or subject masks; replace the sky; and export to JPEG, PNG, or 16-bit TIFF. Edits live in a JSON sidecar, and the AI tools are plugins built on the same API anyone can use. v0.2 added AI denoise, run on the full-size photo in the background, and sky replacement that relights the rest of the scene to match. See the [release notes](release-notes.md) for details and known limitations.

## Ground rules for every release

These come from the project's goals and apply to every item below.

- **Clean-room.** Features are built from public research, open models, and our own work. No code, presets, assets, or model weights from commercial photo editors.
- **Local-first.** Everything runs on the user's machine. No account, no telemetry. The only network use is downloading a model or other asset when the user asks.
- **Non-destructive and open.** New features are new steps in the [sidecar](sidecar-format.md), documented when they're added. Older sidecars keep opening, and a step the app can't render is kept and reported, never dropped.
- **AI as plugins.** Each AI feature is a plugin with a swappable model, written against the public [plugin API](plugins.md).
- **Audited licenses.** Every new dependency and model goes into [LICENSES.md](../LICENSES.md) in the same PR, following its rules. Models with unclear or non-commercial terms are never bundled; at most they're an opt-in download with a notice, as ISNet is today.
- **Three platforms.** Each release works on Windows, macOS, and Linux.

## 0.2.x: patches

Fixes for whatever the first users run into, with nothing new in the edit format.

- Fixes from testing on macOS (HEIC, including portrait-mode photos, and the five-minute end-to-end edit) and from early bug reports.
- Subject masks on the GPU on macOS. Core ML can't convert BiRefNet yet, so they run on the CPU; options include a Core ML-friendly export of the model or another execution provider.
- GPU inference on Linux, where ONNX Runtime has a provider for the user's hardware.
- Replacing ISNet with a comparably fast subject model whose weights and training data are clearly licensed, as soon as one is available.
- Denoise speed: measure it on macOS and Windows GPUs, and tune the tile size.
- RAW denoise before demosaicing, if a clearly licensed model trained on raw sensor data becomes available.

## 0.3: portraits

- **Face-aware retouching:** skin smoothing that keeps texture, eye brightening, and teeth whitening, each as its own adjustment with a strength slider. It needs a local face and facial-feature model with a clear license.
- **Background blur:** a lens-style blur behind the subject, using the subject and background masks Aperio already computes, with an optional depth model for a gradual falloff.

## 0.4: workflow

- **Presets** that save a set of adjustments to apply to other photos, stored in a documented format, plus an original preset library made for Aperio.
- **Copy and paste edits** between photos, choosing which parts to copy (for example, tone but not crop).
- **Batch export** of a selection from the library with the same export settings.
- **History and snapshots:** a visible edit history and named versions of a photo, saved in the sidecar so they survive closing the app.

## Later

- **AI relighting of the whole scene:** moving or adding light sources and changing the direction and color of light, beyond what matching a new sky needs.
- **A plugin ecosystem:** loading plugins from outside the build (dynamic libraries or WebAssembly) once the API has settled, so plugins can be installed without rebuilding the app, and more example plugins.
- Smaller items that will likely land along the way: wide-gamut export (Display P3, Adobe RGB) and ICC profiles, Intel Mac builds, and signed installers.

## Out of scope

Cloud sync, mobile apps, a plugin marketplace, generative fill, and catalog management (DAM) stay out of scope unless the project's goals change.
