# Writing a plugin

Plugins add **operations** (steps that change pixels) and **masks** (weights that choose where a layer applies). Aperio's own AI features, such as sky and subject segmentation, are plugins written against the same API described here.

The API lives in [`crates/aperio-plugin`](../crates/aperio-plugin/src/lib.rs). [`plugins/vignette`](../plugins/vignette/src/lib.rs) is a complete example that uses nothing else, so it's a good starting point to copy.

## How plugins are loaded

For v0.1, a plugin is a Rust crate compiled into the app. The app and the `aperio` command line tool each register their bundled plugins with a `Host` at startup. Loading plugins from outside the build (dynamic libraries or WebAssembly) will come once this API has settled. `API_VERSION` is bumped whenever the trait changes in a way that breaks existing plugins.

To bundle a plugin, add the crate to the workspace and register it in `plugins()` in `crates/aperio-cli/src/main.rs` and `app/src-tauri/src/lib.rs`.

## The `Plugin` trait

```rust
pub trait Plugin: Send + Sync {
    fn info(&self) -> PluginInfo;
    fn apply(&self, params: &Value, image: ImageMut<'_>) -> Result<()>;   // optional
    fn mask(&self, params: &Value, source: Image<'_>) -> Result<Vec<f32>>; // optional
}
```

- **`info`** describes the plugin:
  - `id`: reverse-domain, such as `org.example.grain`.
  - `version`: semantic versioning.
  - `license`: an SPDX license id.
  - `operation` / `mask`: what the plugin provides, each with a UI label and a list of `ParamSpec`s (numbers, booleans, choices, file paths). The app builds controls from these, so a plugin doesn't need any UI code.
  - `models`: every model file the plugin uses, with its license.
- **`apply`** changes pixels in place. Images are linear-light RGBA with sRGB primaries (values can go above 1), stored row by row. The renderer gives the operation a copy of the layer's input and blends the result back through the layer's mask and opacity, so an operation can change every pixel. Inside a layer, plugin operations run in document order, between the built-in steps around them.
- **`mask`** returns one weight per pixel (0 = not applied, 1 = fully applied). It receives the decoded photo *before* any adjustments, so the mask doesn't move as the user edits. The host caches results by plugin, parameters, and image, so a slow mask (such as a neural network) runs once per photo and preview size, not on every slider move.

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
