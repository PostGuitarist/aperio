The first release of Aperio, a free, open-source photo editor that runs entirely on your own computer. It's an early preview: expect rough edges, and keep backups of anything important (Aperio never changes your originals, but it's new software).

New to Aperio? [Getting started](https://github.com/PostGuitarist/aperio/blob/main/docs/getting-started.md) walks through a first edit, from opening a photo to exporting it, in about five minutes.

## What's in it

- **Open** RAW files from most cameras (CR2, CR3, NEF, ARW, RAF, ORF, RW2, DNG, PEF, SRW), JPEG, PNG, TIFF, and HEIC (using your system's own HEIC decoder), with a GPU-accelerated live preview.
- **Library** that reads your folders in place, with thumbnails and camera details. There's no import step and no catalog.
- **Adjust** exposure, contrast, highlights, shadows, whites, blacks, white balance, saturation, and vibrance, and edit tone curves.
- **Crop and straighten.**
- **Layers** with their own adjustments and opacity, masked with a brush, a linear gradient, or a radial gradient.
- **AI sky and subject masks**, computed on your machine with open models, and **sky replacement** with a sky photo of your choice. The models (about 400 MB) are downloaded once, when you ask, and start working on each photo as soon as it opens. An optional fast subject model brings subject masks down to a couple of seconds; its weights don't state a license and were trained on non-commercial data, so it's a separate download with a notice ([details](https://github.com/PostGuitarist/aperio/blob/main/LICENSES.md)).
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
- On macOS, subject masks run on the CPU (Core ML can't run the subject model yet); sky masks use the GPU. Linux runs AI masks on the CPU.
- Exports are sRGB only.
- HEIC on Windows needs Microsoft's HEIF Image Extensions and HEVC Video Extensions from the Microsoft Store. On Linux it needs libheif and its HEVC plugin (`sudo apt install libheif1 libheif-plugin-libde265` on Debian or Ubuntu).

Licenses for every dependency and model are listed in [LICENSES.md](https://github.com/PostGuitarist/aperio/blob/main/LICENSES.md).
