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
| `layers` | Applied bottom to top. The first layer works on the decoded source image. |
| `layers[].enabled`, `opacity` | Default to `true` and `1.0` when missing. |
| `layers[].mask` | Optional. Without a mask the layer applies to the whole image. |
| `layers[].operations` | Applied in order. |

## Operations

Each operation has an `op` tag. Built-in operations:

| `op` | Parameters |
|---|---|
| `exposure` | `ev` in stops |
| `contrast` | `amount`, -1 to 1 |
| `white_balance` | `temperature_k`, `tint` |
| `color` | `saturation`, `vibrance`, each -1 to 1 |
| `curve` | `channel` (`rgb`, `red`, `green`, `blue`, `luma`), `points` as `[x, y]` pairs in 0–1 |
| `crop` | `x`, `y`, `width`, `height` normalized to the source, `angle_deg` |
| `plugin` | `plugin` id, `version`, free-form `params` |

## Masks

Each mask has a `kind` tag. Coordinates are normalized (0–1) to the uncropped source, so masks stay put when the crop changes.

| `kind` | Parameters |
|---|---|
| `brush` | `strokes`: each has `points`, `size`, `feather`, `flow`, `erase` |
| `linear` | `start`, `end` |
| `radial` | `center`, `radius` (x, y), `angle_deg`, `feather`, `invert` |
| `plugin` | `plugin` id, `version`, free-form `params` (e.g. AI segmentation) |

Plugin masks store only their inputs. The generated mask is cached elsewhere and rebuilt when missing, so sidecars stay small and readable.

## Compatibility rules

- Adding an optional field does not change the version.
- Renaming, removing, or changing the meaning of a field bumps `version`, and the reader gains an upgrade step from the old version.
- An operation or mask from a plugin that isn't installed is kept as-is and shown as unavailable, never dropped.
