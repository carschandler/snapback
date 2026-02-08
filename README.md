# `snapback`

Restore EXIF metadata (dates, GPS coordinates) and caption overlays to
exported Snapchat memories.

Snapchat data exports strip EXIF metadata from your photos and videos and
provide it separately in a JSON file. Caption overlays are stored as separate
WebP images (misleadingly named `.png`). `snapback` reads the metadata JSON,
applies it back to your media via `exiftool`, composites caption overlays via
`ffmpeg`, and moves the processed files to a unified output directory.

## Quickstart

1. Request a data export from the [My Data](https://accounts.snapchat.com/v2/download-my-data)
   page in the Snapchat web application. When selecting export options, you must
   choose "Export your Memories" and "Export JSON Files" **at minimum**
   (`snapback` does not currently support exported chat media, but adding support
   for this shouldn't be super complicated; PRs welcome)
2. Once you receive an email notification that your export is ready, return to
   the "My Data" page above and download all of the `.zip` files.
3. Run the following commands:

```
mkdir ~/path/to/snapchat_export
cd ~/path/to/snapchat_export
mv ~/Downloads/mydata~*.zip .
# If you're tight on space you can try to find and extract the json directory
# first and then unzip the archives one at a time, running snapback after each
# unzip. By default, it will unzip all the archives in the current directory.
nix run github:carschandler/snapback -- --help
# Read the help menu and decide how many processes you want to run
# simultaneously & how to handle captions
nix run github:carschandler/snapback -- --processes 3
```

### Caption modes

Snapchat splits captions into their own images when exporting. Choose how to
handle these files using the `--caption` option:

- **ignore**: skip caption overlays entirely, only move the originals to `--output-dir`
- **copy**: create a `_captioned` copy and move both it and the original to `--output-dir`
- **overwrite**: apply the caption directly to the original file

### Processes

If you aren't sure how many processes your system can handle, don't push it too
far; you'll reach a point of diminishing returns. On an M3 Pro chip, I'm just
using 5 and my computer does get a bit toasty after a while. When in doubt, just
omit this argument and a single process will be used, then just leave it running
for a while.

## Prerequisites

*If using `nix`, all runtime dependencies are bundled automatically.* Otherwise,
ensure the following are installed:

- [exiftool](https://exiftool.org/)
- [ffmpeg](https://ffmpeg.org/) (headless variant is fine)
- [unzip](https://infozip.sourceforge.net/)

While many versions of these tools may work, this package has only been tested
using `exiftool v13.39`, `ffmpeg v8.0.1`, `unzip v6.00`).

## Installation

### ❄️ Nix Flake (recommended)

Run directly without installing:

```sh
nix run github:carschandler/snapback -- --help
```

### 🦀 Cargo

```sh
cargo install --git https://github.com/carschandler/snapback
```

Ensure `exiftool`, `ffmpeg`, and `unzip` are on your `PATH` 

### From source

```sh
git clone https://github.com/carschandler/snapback
cd snapback
cargo run --release -- --help
```

## Usage

1. Download your data from Snapchat (Settings > My Data).
2. Place the `.zip` files in a directory.
3. Run snapback

This will unzip the archives, apply EXIF metadata, handle captions, and move
processed media to `./processed_media` by default.

### Options

| Flag | Description | Default |
|------|-------------|---------|
| `-m, --memories-history-json-path` | Path to `memories_history.json` | `./json/memories_history.json` |
| `-z, --zip-dir` | Directory containing `.zip` files to unpack | `.` |
| `-p, --processes` | Number of concurrent exiftool/ffmpeg processes | `1` |
| `-o, --output-dir` | Directory to move processed files into | `./processed_media` |
| `--media-prefix` | Glob prefix for media directories | `memories` |
| `-c, --captions` | Caption overlay mode: `ignore`, `copy`, `overwrite` | `copy` |
