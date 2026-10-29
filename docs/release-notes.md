Aperio 0.2 adds two AI tools: denoise for high-ISO photos, and sky replacement that relights the rest of the scene to match the new sky. Like everything else in Aperio, both run entirely on your own computer. It's still an early preview: expect rough edges, and keep backups of anything important (Aperio never changes your originals, but it's new software).

New to Aperio? [Getting started](https://github.com/PostGuitarist/aperio/blob/main/docs/getting-started.md) walks through a first edit, from opening a photo to exporting it, in about five minutes.

## What's new in 0.2

- **AI denoise.** Tick **AI denoise** in the Adjust tab's new Noise group to remove grain from high-ISO photos, with **Amount** and **Keep detail** sliders. It runs NAFNet, an open model (MIT) trained on real smartphone noise, on the full-size photo in the background: you keep editing while it works, the status bar shows its progress, and after that the sliders respond straight away. Expect a few minutes for a 24-megapixel photo on a laptop CPU, less on a GPU.
- **Sky replacement that relights the scene.** Replacing the sky now also adds a **Match scene to sky** layer that shifts the rest of the photo's colour and light toward the new sky, tints water and reflections with it, and lays a soft band of its haze along the horizon. Each part has its own slider. It's colour matching driven by the AI masks; nothing is generated.
- **A simpler AI tab.** The tools (Select, Sky, Effects) come first. The model download is one short card until it's done, and the per-model details and the optional fast subject model are folded under **Models** at the bottom.
- **Plugins can clean up the full-size photo.** The plugin API (version 2) adds *source steps*, which run once on the full-size photo before any layer, with progress and cancelling. Denoise is built on it ([docs/plugins.md](https://github.com/PostGuitarist/aperio/blob/main/docs/plugins.md)). Plugins written for 0.1 need one added line.
- The standard AI download is now about 520 MB, because it includes the denoise model. If you already have the 0.1 models, the AI tab offers just the new one.

Sidecars from 0.1 open unchanged, and a 0.1 sidecar edited in 0.2 still opens in 0.1 (any denoise or relighting step is kept there and reported as skipped).

## Everything in Aperio

- **Open** RAW files from most cameras (CR2, CR3, NEF, ARW, RAF, ORF, RW2, DNG, PEF, SRW), JPEG, PNG, TIFF, and HEIC (using your system's own HEIC decoder), with a GPU-accelerated live preview.
- **Library** that reads your folders in place, with thumbnails and camera details. There's no import step and no catalog.
- **Adjust** exposure, contrast, highlights, shadows, whites, blacks, white balance, saturation, and vibrance, edit tone curves, and **remove noise** with AI denoise.
- **Crop and straighten.**
- **Layers** with their own adjustments and opacity, masked with a brush, a linear gradient, or a radial gradient.
- **AI sky and subject masks**, computed on your machine with open models, and **sky replacement** with a sky photo of your choice, relighting the scene to match. The models (about 520 MB) are downloaded once, when you ask, and start working on each photo as soon as it opens. An optional fast subject model brings subject masks down to a couple of seconds; its weights don't state a license and were trained on non-commercial data, so it's a separate download with a notice ([details](https://github.com/PostGuitarist/aperio/blob/main/LICENSES.md)).
- **Non-destructive editing.** Every edit is saved to a readable JSON sidecar next to the photo, with undo and redo.
- **Export** to JPEG, PNG, or 16-bit TIFF, with resizing, output sharpening, and a choice of which metadata to copy.
- **Plugins.** The AI tools are plugins built on a documented API, with an example plugin to start from ([docs/plugins.md](https://github.com/PostGuitarist/aperio/blob/main/docs/plugins.md)).

## Installing

These builds aren't signed with a paid developer certificate yet, so your system will warn you the first time.

- **macOS (Apple Silicon):** open the `.dmg` and drag Aperio to Applications. The first time, macOS says it can't verify the app: open **System Settings → Privacy & Security** and click **Open Anyway**.
- **Windows (64-bit):** run the `-setup.exe` (or the `.msi`). If SmartScreen appears, click **More info → Run anyway**.
- **Linux:** use the `.AppImage` (make it executable and run it), the `.deb`, or the `.rpm`. It needs glibc 2.38 or newer (Ubuntu 24.04, Debian 13, Fedora 39 or later).

## Known limitations

Please report problems on [Issues](https://github.com/PostGuitarist/aperio/issues), with the photo type and your system.


- Intel Macs aren't supported by this build.
- On macOS, subject masks run on the CPU (Core ML can't run the subject model yet); sky masks use the GPU. Linux runs AI masks and denoise on the CPU.
- Denoise works on the demosaiced photo, not on RAW sensor data, and its model was trained on smartphone noise, so very noisy RAW files from large sensors may keep some grain.
- Sky relighting matches colour and brightness only; it doesn't move shadows or light direction.
- Exports are sRGB only.
- HEIC on Windows needs Microsoft's HEIF Image Extensions and HEVC Video Extensions from the Microsoft Store. On Linux it needs libheif and its HEVC plugin (`sudo apt install libheif1 libheif-plugin-libde265` on Debian or Ubuntu).

Licenses for every dependency and model are listed in [LICENSES.md](https://github.com/PostGuitarist/aperio/blob/main/LICENSES.md).
