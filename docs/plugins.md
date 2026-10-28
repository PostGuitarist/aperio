# Writing a plugin

Plugins add **operations** (steps that change pixels), **masks** (weights that choose where a layer applies), and **source steps** (slow clean-ups, such as denoising, that run once on the full-size photo before any layer). Aperio's own AI features, such as sky and subject segmentation, are plugins written against the same API described here.

The API lives in [`crates/aperio-plugin`](../crates/aperio-plugin/src/lib.rs). [`plugins/vignette`](../plugins/vignette/src/lib.rs) is a complete example that uses nothing else, so it's a good starting point to copy.

## How plugins are loaded

For now, a plugin is a Rust crate compiled into the app. The app and the `aperio` command line tool each register their bundled plugins with a `Host` at startup. Loading plugins from outside the build (dynamic libraries or WebAssembly) will come once this API has settled. `API_VERSION` is bumped whenever the trait changes in a way that breaks existing plugins. Version 2 (Aperio 0.2) added source steps, so plugins written for version 1 need a `source: None` line in their `PluginInfo`.

To bundle a plugin, add the crate to the workspace and register it in `plugins()` in `crates/aperio-cli/src/main.rs` and `app/src-tauri/src/lib.rs`.

## The `Plugin` trait

```rust
pub trait Plugin: Send + Sync {
    fn info(&self) -> PluginInfo;
    fn apply(&self, params: &Value, image: ImageMut<'_>) -> Result<()>;   // optional
    fn apply_masked(&self, params: &Value, image: ImageMut<'_>,
                    mask: Option<&[f32]>) -> Result<()>;                // optional
    fn source(&self, params: &Value, image: ImageMut<'_>,
              progress: Progress<'_>) -> Result<()>;                   // optional
    fn mask(&self, params: &Value, source: Image<'_>) -> Result<Vec<f32>>; // optional
}
```

- **`info`** describes the plugin:
  - `id`: reverse-domain, such as `org.example.grain`.
  - `version`: semantic versioning.
  - `license`: an SPDX license id.
  - `operation` / `mask` / `source`: what the plugin provides, each with a UI label and a list of `ParamSpec`s (numbers, booleans, choices, file paths). The app builds controls from these, so a plugin doesn't need any UI code.
  - `models`: every model file the plugin uses, with its license.
- **`apply`** changes pixels in place. Images are linear-light RGBA with sRGB primaries (values can go above 1), stored row by row. The renderer gives the operation a copy of the layer's input and blends the result back through the layer's mask and opacity, so an operation can change every pixel. Inside a layer, plugin operations run in document order, between the built-in steps around them.
- **`apply_masked`** is `apply` with the layer's mask, for operations that need to know where they will show (`None` when the layer has no mask, or a hand-drawn one). By default it calls `apply`, so most plugins only write `apply`.
- **`source`** changes the full-size photo in place, once, before any layer runs. Use it for work that needs every pixel at full resolution and is too slow to repeat on each slider move, such as an AI denoiser. A source step is stored in the sidecar like any plugin operation (normally on the base layer), but it runs wherever it is, in document order, ignoring its layer's mask and opacity; turning its layer off turns it off. The host keeps the result for the photo and parameters, and the preview, masks, and export all start from it. Call `progress.report(share_done)?` now and then: it lets the app show a progress bar, and returns `Err(Cancelled)` when the user moves on so the plugin can stop early.
- **`mask`** returns one weight per pixel (0 = not applied, 1 = fully applied). It receives the decoded photo *before* any adjustments (but after source steps), so the mask doesn't move as the user edits. The host caches results by plugin, parameters, and image, so a slow mask (such as a neural network) runs once per photo and preview size, not on every slider move.

Read parameters through your `ParamSpec`s (`read_number`, `read_bool`, `read_choice`, `read_path`). These fill in defaults for missing keys and clamp numbers to their range, so older sidecars keep working when you add a parameter.

Rendering runs on preview-sized images and on the full-size export. Measure distances relative to the image size, as the vignette does, so a preview and its export look the same.

## In the sidecar

```json
{ "op": "plugin", "plugin": "org.aperio.example.vignette", "version": "1.0.0",
  "params": { "amount": -0.4, "midpoint": 0.5 } }
```

```json
"mask": { "kind": "plugin", "plugin": "org.aperio.segment", "version": "0.1.0",
          "params": { "class": "sky" } }
```

A step is rendered by an installed plugin with the same id whose version is compatible with the version in the sidecar:
- for 1.x and later, the same major version;
- for 0.x, the same minor version;
- in both cases, at least as new as the version in the sidecar.

If no such plugin is installed, the step is left out and reported, and it stays in the sidecar. Opening a photo on a machine without the plugin never loses the edit.

## Licenses

Plugins bundled with Aperio must use a license compatible with GPL-3.0-or-later. Every model they use, and its training data, must be listed in [LICENSES.md](../LICENSES.md), following the rules there.

## The bundled AI plugin

`org.aperio.segment` ([`plugins/segment`](../plugins/segment/src/lib.rs)) provides the AI sky and subject masks. It runs open ONNX models locally with ONNX Runtime. The network sees a small copy of the photo (320 px for sky, 1024 px for subjects). Its output is scaled back up and refined with a guided filter, so edges follow branches and rooflines in the full-size photo.

Models are kept in the platform's data folder (for example `~/.local/share/aperio/models`, or `$APERIO_MODELS_DIR`). They're downloaded only when the user asks:

```sh
aperio models            # what's installed
aperio models download   # fetch and verify missing models (~520 MB)
```

A layer whose model isn't downloaded is reported as skipped. It stays in the sidecar.

When a photo is opened and the models are downloaded, the app starts them on its preview in the background (quickest first), so AI masks are usually ready by the time they're added: a mask whose model has already run takes well under a second. The plugin keeps the networks' last few outputs, so a subject mask and its inverse (the background) share one run.

The models (for this plugin and for denoise) run on the GPU where ONNX Runtime has a provider for it: DirectML on Windows and Core ML on macOS (the GPU and Neural Engine). If the GPU can't take a model, it runs on the CPU instead, and a model Core ML can't convert is remembered in a `<model>.coreml.unsupported` file next to it, because Core ML can spend a minute trying before it gives up. Delete the file to try again, for example after an update. For now Core ML takes the sky model but not BiRefNet, so subject masks run on the CPU on macOS. Linux runs on the CPU for now. Set `APERIO_AI_CPU=1` to skip the GPU, for example to compare results or work around a driver problem. To see where the models run and how long a mask takes:

```sh
aperio models bench photo.jpg
```

ISNet general use is a much faster subject model (about 1.5 s on a 4-core CPU against BiRefNet's 15–20 s), with softer edges. Its weights don't state a license and its training data is non-commercial (see [LICENSES.md](../LICENSES.md)), so it's never part of "Download models": it's a separate download with its own notice (`aperio models download isnet_general_use`), and once it's downloaded, subject layers get a Best/Fast switch. A layer set to Fast uses BiRefNet where ISNet isn't downloaded or the build leaves it out.

ISNet comes from the segment plugin's `fast-subject` Cargo feature. The app and the command line tool turn it on by default; build them with `--no-default-features` to leave it out entirely, for example for a distribution that only ships clearly licensed models:

```sh
cargo run -p aperio-cli --no-default-features -- models
cd app && npm run tauri build -- -- --no-default-features
```

## AI denoise

`org.aperio.denoise` ([`plugins/denoise`](../plugins/denoise/src/lib.rs)) is a source step: it runs once on the full-size photo, before any layer. The network is [NAFNet](https://github.com/megvii-research/NAFNet) trained on [SIDD](https://www.eecs.yorku.ca/~kamel/sidd/), real noise from smartphone photos at many ISO settings; its code, weights, and training data are all MIT. Nobody published an ONNX export of it, so Aperio makes one with [`tools/export-nafnet.py`](../tools/export-nafnet.py) in the Models workflow and publishes it as a release asset.

The network sees the photo in 512 px tiles that overlap by 32 px and are feathered together, so memory stays bounded at any size and no seams show. It sees sRGB values from 0 to 1, so highlights above 1 (from RAW files) keep their brightness: what the network changes is added to the photo rather than replacing it. Two sliders mix its output with the photo: **Amount**, and **Keep detail**, which brings back fine brightness detail and grain without the colour noise. The plugin keeps the network's output for the last photo, so the sliders don't run it again.

A 512 px tile takes about 2 seconds on a 4-core CPU, so a 24-megapixel photo takes a few minutes there, and much less on a GPU. `aperio models bench` times it on your machine.

NAFNet-SIDD was trained on sRGB images, so denoise runs after RAW files are demosaiced. Denoising RAW data before demosaicing could do better and needs a model trained on raw sensor data; that's left for later.

## Sky replacement

`org.aperio.sky` ([`plugins/sky`](../plugins/sky/src/lib.rs)) is an operation that paints a sky photo of the user's choice; the layer's mask (normally the AI sky mask) decides where it shows. It uses `apply_masked` to fit the new sky between the top of the frame and the horizon the mask shows, and it leaves out the bottom of the sky photo once its own ground or skyline starts. Along the skyline, where the AI mask is unsure, pixels that look like the old sky are blended in as more sky than the mask says, so haze and gaps between leaves don't show a pale fringe.

### Matching the scene to the new sky

`org.aperio.sky.relight` ([`plugins/sky/src/relight.rs`](../plugins/sky/src/relight.rs)) makes the rest of the photo match the new sky. The sky operation only shows where the sky mask is, so this is a separate operation on a companion layer masked to everything but the sky (the AI sky mask, inverted); the app adds both layers when you replace the sky and keeps their sky photos in step. It compares the old sky (the part of the photo outside its mask) with the new sky photo, then:

- **Match lighting** moves the scene's colour all the way, and its brightness half the way, toward the new sky's light, more in bright areas than in shadows, with each channel's change limited to 3x either way.
- **Reflections** tints areas below the horizon that look like the old sky (water, wet ground, glass) with the colour of the new sky near its horizon, keeping their texture.
- **Horizon haze** lays a little of that colour in a soft band around the horizon, so the skyline sits in the new air.

It's colour matching driven by the masks, nothing generative. Lighting that changes direction, such as new shadows, is a later step on the [roadmap](ROADMAP.md).
