# Sidecar format (version 1)

Aperio never writes to an original photo. Every edit lives in a JSON sidecar saved next to it. The Rust types in [`crates/aperio-core`](../crates/aperio-core/src) are the reference implementation of this document.

## File name

The sidecar takes the full original file name plus `.aperio.json`:

```
IMG_0001.CR3            original, never modified
IMG_0001.CR3.aperio.json  edits
```

Keeping the original extension means `IMG_0001.CR3` and `IMG_0001.JPG` from the same shot get separate sidecars.

## Example

```json
{
  "format": "aperio.edit",
  "version": 1,
  "source": {
    "file_name": "IMG_0001.CR3",
    "size_bytes": 25000000,
    "sha256": "9f86d08..."
  },
  "layers": [
    {
      "name": "Base",
      "enabled": true,
      "opacity": 1.0,
      "operations": [
        { "op": "exposure", "ev": 0.5 },
        { "op": "white_balance", "temperature_k": 5600, "tint": 0.0 },
        { "op": "curve", "channel": "rgb", "points": [[0, 0], [0.5, 0.55], [1, 1]] }
      ]
    },
    {
      "name": "Sky",
      "mask": {
        "kind": "plugin",
        "plugin": "org.aperio.segment",
        "version": "0.1.0",
        "params": { "class": "sky" }
      },
      "operations": [{ "op": "exposure", "ev": -0.7 }]
    }
  ]
}
```

## Fields

| Field | Meaning |
|---|---|
| `format` | Always `"aperio.edit"`. Readers reject anything else. |
| `version` | Schema version. Readers reject versions newer than they support and upgrade older ones. |
| `source.file_name` | Original file name only, so the pair survives a folder move. |
| `source.size_bytes`, `source.sha256` | Used to warn when the original has been replaced. `sha256` is optional. |
| `rating`, `flag`, `label` | Optional Library marks, left out when unset. `rating` is 0–5; `flag` is `pick` or `reject`; `label` is `red`, `yellow`, `green`, `blue`, or `purple`. A sidecar holding only marks doesn't count as an edit. |
| `snapshots` | Optional, left out when empty. Named versions of the edit: each has a `name`, `created_ms` (milliseconds since the Unix epoch), and its own `layers`, in the same form as the top-level `layers`. Snapshots are never rendered. |
| `layers` | Applied bottom to top. Each layer runs its operations on the result of the layers below, then blends that back in by mask × opacity. The first layer works on the decoded source image. |
| `layers[].enabled`, `opacity` | Default to `true` and `1.0` when missing. |
| `layers[].blend_mode` | Optional, `normal` when missing (and not written then). One of `normal`, `multiply`, `screen`, `overlay`, `soft_light`, `color`, `luminosity`. The layer's result is combined with what's below in this mode, then blended in by mask × opacity. Multiply, screen, overlay, and soft light work on sRGB-encoded values; color keeps the brightness below and takes the layer's color, and luminosity does the opposite. With a mode other than `normal`, a layer with no operations still changes the photo. |
| `layers[].mask` | Optional. Without a mask the layer applies to the whole image. |
| `layers[].operations` | Applied in order. |

## Operations

Each operation has an `op` tag. Built-in operations:

| `op` | Parameters |
|---|---|
| `exposure` | `ev` in stops |
| `contrast` | `amount`, -1 to 1 |
| `tone` | `highlights`, `shadows`, `whites`, `blacks`, each -1 to 1 and 0 when missing. Highlights and shadows move the bright and dark midtones; whites and blacks move the ends of the range. Runs as a hue-keeping (luma) curve on sRGB-encoded values; each negative amount exactly undoes the matching positive one. |
| `white_balance` | `temperature_k`, `tint` |
| `color` | `saturation`, `vibrance`, each -1 to 1 |
| `presence` | `texture`, `clarity`, `dehaze`, each -1 to 1 and 0 when missing. Texture boosts (or smooths) fine detail and clarity larger shapes in the midtones, both on brightness only. Dehaze uses a dark-channel estimate of haze to remove it, or adds haze when negative. Neighborhood sizes are fractions of the image's longer edge, so previews and full-size exports match. |
| `hsl` | `hue`, `saturation`, `luminance`: arrays of eight values from -1 to 1, one per color band (red, orange, yellow, green, aqua, blue, purple, magenta), missing arrays all 0. A full hue value turns the band 30°. Neighboring bands blend smoothly, and greys are left alone. |
| `color_grade` | `shadows`, `midtones`, `highlights`, `global`: wheels with `hue` (degrees), `saturation` (0–1) and `luminance` (-1 to 1). `blending` (0–1, default 0.5) sets how much the tonal ranges overlap; `balance` (-1 to 1, default 0) moves the split between shadows and highlights. Tints shift color without changing brightness. |
| `curve` | `channel` (`rgb`, `red`, `green`, `blue`, `luma`), `points` as `[x, y]` pairs in 0–1. Points are joined with a monotone cubic and act on sRGB-encoded values; `luma` scales all three channels together so hue is kept. |
| `crop` | `x`, `y`, `width`, `height` normalized to the oriented source, and `angle_deg` rotating the rectangle around its center. Applied after every other operation; only the last crop counts. |
| `plugin` | `plugin` id, `version`, free-form `params`. Runs in order with the layer's other operations on the layer's input; the result is blended through the layer's mask. A plugin that declares the step a *source step* (such as denoise) instead runs it once on the full-size photo before any layer, ignoring the layer's mask and opacity. See [plugins.md](plugins.md). |

## Masks

Each mask has a `kind` tag. Coordinates are normalized (0–1) to the uncropped source, so masks stay put when the crop changes.

| `kind` | Parameters |
|---|---|
| `brush` | `strokes`, replayed in order. Each has `points` (a polyline), `size` (brush diameter as a fraction of the image's longer edge), `feather` (0 = hard edge, 1 = soft all the way to the center), `flow` (0–1 strength), and `erase` (remove instead of add). |
| `linear` | `start` (fully applied) to `end` (not applied), with a smooth falloff between. |
| `radial` | `center`, `radius` (x, y), `angle_deg`, `feather` (fraction of the radius that fades out), `invert` |
| `luminance` | `low`, `high` (sRGB-encoded luminance, 0–1): fully selected between them, fading out over `feather` (default 0.1) on either side. |
| `color` | `hue` (degrees, 0 = red, 120 = green, 240 = blue): fully selected within `range` degrees (default 20), fading out over `feather` more degrees (default 20). Greys are never selected. |
| `combine` | `parts`, each with a `mask`, a `mode` (`add`, `subtract`, or `intersect`; default `add`), and an optional `invert`. The first part starts the result whatever its mode; each later part is added (a + b − ab), subtracted (a × (1 − b)), or intersected (a × b). Parts can be any kind, including another `combine`. |
| `plugin` | `plugin` id, `version`, free-form `params` (e.g. AI segmentation) |

Luminance and color masks, like plugin masks, are read from the photo before any layer's adjustments, so editing a layer never changes what a range mask selects.

Plugin masks store only their inputs. The generated mask is cached elsewhere and rebuilt when missing, so sidecars stay small and readable. The AI masks keep their network outputs in the user's cache folder (for example `~/Library/Caches/aperio/masks` on macOS, `~/.cache/aperio/masks` on Linux), up to 512 MB, so a photo opened again later doesn't wait for the network. Deleting that folder is always safe.

## Presets

User presets are separate files, one per preset, named `<name>.aperio-preset.json` in Aperio's config folder (for example `~/.config/aperio/presets` on Linux, `~/Library/Application Support/aperio/presets` on macOS):

```json
{
  "format": "aperio.preset",
  "version": 1,
  "name": "Golden hour",
  "operations": [
    { "op": "white_balance", "temperature_k": 8000, "tint": 0.03 },
    { "op": "color", "saturation": 0.0, "vibrance": 0.15 }
  ]
}
```

`operations` uses the same operation objects as a layer. Applying a preset replaces the layer's operations of the same kind (one per kind, and one curve per channel) and keeps everything else. Crop and plugin operations are never saved in a preset.

## Albums and smart collections

Albums and smart collections are kept in one file, `collections.json`, in the same config folder as presets. An album lists photo paths; a smart collection stores its rules (`min_rating`, and optionally `flag`, `label`, `edited`, `kind`, and `name_contains`) and is worked out each time from the photos in the Library's recent folders.

## Compatibility rules

- Adding an optional field does not change the version.
- Renaming, removing, or changing the meaning of a field bumps `version`, and the reader gains an upgrade step from the old version.
- An operation or mask from a plugin that isn't installed (or whose installed version is incompatible) is kept as-is and reported as skipped, never dropped. A plugin mask that can't be generated leaves its whole layer out.
