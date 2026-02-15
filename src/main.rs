use clap::{Parser, ValueEnum};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use chrono::{DateTime, NaiveDateTime, Utc};
use glob::glob;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use ripunzip::{NullProgressReporter, UnzipEngine, UnzipOptions};
use serde::Deserializer;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, ValueEnum)]
enum OverlayMode {
    /// Apply overlay directly to the original file
    Overwrite,
    /// Create an _overlaid copy while preserving the original
    Copy,
    /// Skip overlays entirely
    Ignore,
}

#[derive(Debug, Parser)]
#[command(
    version,
    about,
    long_about = "\
Restore metadata and overlays to Snapchat memory exports.\n\n\
Snapback processes a Snapchat data export by:\n\n\
1. Unzipping exported archive(s) (by default unzips all .zip files in the --zip-dir)\n\
2. Parsing memories_history.json for dates and GPS coordinates\n\
3. Writing EXIF/metadata back onto each photo and video via exiftool\n\
4. Optionally compositing overlay PNGs (captions, stickers, drawings) onto\n\
   the original media using ffmpeg\n\
5. Moving the processed files into an output directory\n\n\
External dependencies: exiftool, ffmpeg"
)]
struct Args {
    /// How to handle overlays (captions, drawings, stickers, etc.)
    #[arg(short, long, value_enum, default_value_t = OverlayMode::Overwrite)]
    overlays: OverlayMode,

    /// Number of concurrent exiftool/ffmpeg processes
    #[arg(short, long, default_value_t = 1)]
    processes: usize,

    /// Directory containing zip files to unpack
    #[arg(short, long, default_value = ".")]
    zip_dir: PathBuf,

    /// Directory to move processed media files into
    #[arg(short = 'd', long, default_value = "./processed_media")]
    output_dir: PathBuf,

    /// Skip the unzip step (use if .zip files are already extracted)
    #[arg(long, default_value_t = false)]
    skip_unzip: bool,

    // Path to the "memories_history.json" file from the export
    #[arg(short = 'j', long, default_value = "./json/memories_history.json")]
    memories_history_json_path: PathBuf,

    /// Directory name prefix to glob for media files (e.g. "memories" matches "memories*/**/*.jpg")
    #[arg(short, long, default_value = "memories")]
    media_prefix: String,
}

fn main() {
    let args = Args::parse();

    // Set up parallel processing
    rayon::ThreadPoolBuilder::new()
        .num_threads(args.processes)
        .build_global()
        .unwrap();

    // Unzip logic (using ripunzip for parallel extraction)
    if !args.skip_unzip {
        let zip_dir = &args.zip_dir;
        let zip_pattern = zip_dir.join("*.zip");
        let zip_pattern_str = zip_pattern.to_str().expect("Invalid zip path pattern");

        println!("Looking for zip files in: {}", zip_pattern_str);

        for entry in glob(zip_pattern_str).expect("Failed to read glob pattern for zips") {
            match entry {
                Ok(path) => {
                    println!("Unzipping {:?}", path);
                    let zip_file = match fs::File::open(&path) {
                        Ok(f) => f,
                        Err(e) => {
                            eprintln!("Failed to open zip {:?}: {}", path, e);
                            continue;
                        }
                    };
                    match UnzipEngine::for_file(zip_file) {
                        Ok(engine) => {
                            let options = UnzipOptions {
                                output_directory: Some(PathBuf::from(".")),
                                password: None,
                                single_threaded: false,
                                filename_filter: None,
                                progress_reporter: Box::new(NullProgressReporter),
                            };
                            match engine.unzip(options) {
                                Ok(()) => println!("Successfully unzipped {:?}", path),
                                Err(e) => eprintln!("Unzip failed for {:?}: {}", path, e),
                            }
                        }
                        Err(e) => eprintln!("Failed to open zip {:?}: {}", path, e),
                    }
                }
                Err(e) => eprintln!("Glob error: {:?}", e),
            }
        }
    }

    // Existing logic
    if !args.memories_history_json_path.exists() {
        eprintln!(
            "Memories history file not found at {:?}. Did unzipping work?",
            args.memories_history_json_path
        );
    }

    // Check if we can proceed
    if !args.memories_history_json_path.exists() {
        return;
    }

    let memories_data = parse_memories_history_file(&args.memories_history_json_path).unwrap();

    let media_map: HashMap<String, Media> = memories_data
        .saved_media
        .into_iter()
        .map(|m| (m.id.clone(), m))
        .collect();

    let patterns = [
        format!("{}*/**/*.jpg", args.media_prefix),
        format!("{}*/**/*.mp4", args.media_prefix),
    ];

    // Collect paths to a vector for parallel iteration
    let paths: Vec<PathBuf> = patterns
        .iter()
        .flat_map(|p| glob(p).expect("Failed to read glob pattern"))
        .filter_map(Result::ok)
        .collect();

    let overlay_mode = &args.overlays;

    let pb = ProgressBar::new(paths.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "Processing {pos}/{len} [{wide_bar:.cyan/blue}] {percent}% ({eta})",
        )
        .unwrap()
        .progress_chars("=> "),
    );

    paths.par_iter().for_each(|path| {
        let file_name_str = path
            .file_name()
            .unwrap_or_default()
            .to_str()
            .unwrap_or("?")
            .to_string();
        let mut did_exif = false;
        let mut did_overlay = false;

        // 1. Apply EXIF metadata
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if let Some(id) = parse_id_from_stem(stem) {
                if let Some(media) = media_map.get(&id) {
                    let date_str = media.date.format("%Y:%m:%d %H:%M:%S").to_string();
                    let lat_str = media.coordinate.lat.to_string();
                    let lon_str = media.coordinate.lon.to_string();

                    let status = Command::new("exiftool")
                        .arg("-overwrite_original")
                        .arg(format!("-DateTimeOriginal={}", date_str))
                        .arg(format!("-GPSLatitude={}", lat_str))
                        .arg(format!("-GPSLatitudeRef={}", lat_str))
                        .arg(format!("-GPSLongitude={}", lon_str))
                        .arg(format!("-GPSLongitudeRef={}", lon_str))
                        .arg("-q")
                        .arg(path)
                        .status();

                    match status {
                        Ok(s) => {
                            if s.success() {
                                did_exif = true;
                            } else {
                                pb.println(format!("ExifTool failed for {:?}", path));
                            }
                        }
                        Err(e) => {
                            pb.println(format!("Failed to execute ExifTool for {:?}: {}", path, e))
                        }
                    }
                }
            }
        }

        // 2. Apply overlay (after EXIF so metadata is already set)
        if !matches!(overlay_mode, OverlayMode::Ignore) {
            if let Some(parent) = path.parent() {
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    let overlay_filename = if file_name.ends_with("-main.jpg") {
                        Some(file_name.replace("-main.jpg", "-overlay.png"))
                    } else if file_name.ends_with("-main.mp4") {
                        Some(file_name.replace("-main.mp4", "-overlay.png"))
                    } else {
                        None
                    };

                    if let Some(overlay_name) = overlay_filename {
                        let overlay_path = parent.join(overlay_name);
                        if overlay_path.exists() {
                            // Overlay files are named .png but contain WebP data;
                            // convert to real PNG for ffmpeg (ffmpeg's native WebP decoder
                            // can't handle lossy VP8 with a separate alpha channel).
                            let converted_overlay = path.with_file_name(format!(
                                "{}_overlay.png",
                                path.file_stem().unwrap().to_str().unwrap()
                            ));
                            let overlay_to_use = match fs::read(&overlay_path) {
                                Ok(bytes) => match image::load_from_memory(&bytes) {
                                    Ok(img) => match img.save(&converted_overlay) {
                                        Ok(()) => &converted_overlay,
                                        Err(e) => {
                                            pb.println(format!(
                                                "Failed to save converted overlay: {}",
                                                e
                                            ));
                                            &overlay_path
                                        }
                                    },
                                    Err(e) => {
                                        pb.println(format!(
                                            "Failed to decode overlay {:?}: {}",
                                            overlay_path, e
                                        ));
                                        &overlay_path
                                    }
                                },
                                Err(e) => {
                                    pb.println(format!(
                                        "Failed to read overlay file {:?}: {}",
                                        overlay_path, e
                                    ));
                                    &overlay_path
                                }
                            };

                            let ext = path.extension().unwrap_or_default().to_str().unwrap_or("");
                            let stem = path.file_stem().unwrap().to_str().unwrap();
                            let (input_path, final_output) = match overlay_mode {
                                OverlayMode::Copy => {
                                    let overlaid = path
                                        .with_file_name(format!("{}_with_overlay.{}", stem, ext));
                                    if let Err(e) = fs::copy(path, &overlaid) {
                                        pb.println(format!(
                                            "Failed to copy {:?} for overlay: {}",
                                            path, e
                                        ));
                                        let _ = fs::remove_file(&converted_overlay);
                                        pb.inc(1);
                                        return;
                                    }
                                    (overlaid.clone(), overlaid)
                                }
                                OverlayMode::Overwrite => (path.to_path_buf(), path.to_path_buf()),
                                OverlayMode::Ignore => unreachable!(),
                            };

                            let temp_output = path.with_file_name(format!("{}_temp.{}", stem, ext));

                            let is_video = file_name.ends_with(".mp4");

                            let mut cmd = Command::new("ffmpeg");
                            cmd.arg("-y")
                                .arg("-loglevel")
                                .arg("error")
                                .arg("-i")
                                .arg(&input_path);

                            if is_video {
                                cmd.arg("-loop").arg("1");
                            }

                            cmd.arg("-i").arg(overlay_to_use);

                            if is_video {
                                cmd.arg("-shortest");
                            }

                            if is_video {
                                cmd.arg("-filter_complex")
                                    .arg("[1:v][0:v]scale=rw:rh[ol];[0:v][ol]overlay=0:0");
                                cmd.arg("-c:a").arg("copy");
                            } else {
                                cmd.arg("-filter_complex")
                                    .arg("[1:v][0:v]scale=rw:rh[ol];[0:v][ol]overlay=0:0");
                                cmd.arg("-pix_fmt").arg("yuvj420p");
                                cmd.arg("-update").arg("1");
                                cmd.arg("-frames:v").arg("1");
                            }

                            cmd.arg(&temp_output);

                            let status = cmd.status();

                            let _ = fs::remove_file(&converted_overlay);

                            match status {
                                Ok(s) => {
                                    if s.success() {
                                        if let Err(e) = fs::rename(&temp_output, &final_output) {
                                            pb.println(format!(
                                                "Failed to finalize overlaid file {:?}: {}",
                                                final_output, e
                                            ));
                                        } else {
                                            did_overlay = true;
                                        }
                                    } else {
                                        pb.println(format!(
                                            "FFmpeg failed for overlay on {:?}",
                                            path
                                        ));
                                        let _ = fs::remove_file(&temp_output);
                                        if matches!(overlay_mode, OverlayMode::Copy) {
                                            let _ = fs::remove_file(&final_output);
                                        }
                                    }
                                }
                                Err(e) => pb
                                    .println(format!("Failed to run FFmpeg for {:?}: {}", path, e)),
                            }
                        }
                    }
                }
            }
        }

        // Log once per file
        match (did_exif, did_overlay) {
            (true, true) => pb.println(format!("Added EXIF data and overlay to {}", file_name_str)),
            (true, false) => pb.println(format!("Added EXIF data to {}", file_name_str)),
            (false, true) => pb.println(format!("Added overlay to {}", file_name_str)),
            (false, false) => {}
        }
        pb.inc(1);
    });

    pb.finish_with_message("Processing complete");

    // 3. Move processed media files to output directory
    let output_dir = &args.output_dir;
    if let Err(e) = fs::create_dir_all(output_dir) {
        eprintln!("Failed to create output directory {:?}: {}", output_dir, e);
        return;
    }

    let move_pb = ProgressBar::new(paths.len() as u64);
    move_pb.set_style(
        ProgressStyle::with_template("Moving {pos}/{len} [{wide_bar:.green/dim}] {percent}%")
            .unwrap()
            .progress_chars("=> "),
    );

    let mut moved = 0usize;
    for path in &paths {
        let file_name = match path.file_name() {
            Some(name) => name,
            None => {
                move_pb.inc(1);
                continue;
            }
        };

        let dest = output_dir.join(file_name);
        match fs::rename(path, &dest) {
            Ok(()) => moved += 1,
            Err(e) => move_pb.println(format!("Failed to move {:?} to {:?}: {}", path, dest, e)),
        }

        // Also move the _overlaid version if it exists (copy mode)
        if matches!(args.overlays, OverlayMode::Copy) {
            let ext = path.extension().unwrap_or_default().to_str().unwrap_or("");
            let stem = path.file_stem().unwrap().to_str().unwrap();
            let overlaid = path.with_file_name(format!("{}_overlaid.{}", stem, ext));
            if overlaid.exists() {
                let overlaid_dest = output_dir.join(overlaid.file_name().unwrap());
                match fs::rename(&overlaid, &overlaid_dest) {
                    Ok(()) => moved += 1,
                    Err(e) => move_pb.println(format!(
                        "Failed to move {:?} to {:?}: {}",
                        overlaid, overlaid_dest, e
                    )),
                }
            }
        }

        move_pb.inc(1);
    }

    move_pb.finish_and_clear();
    println!("Moved {} files to {:?}", moved, output_dir);
}

fn parse_id_from_stem(stem: &str) -> Option<String> {
    // Expected format: YYYY-MM-DD_UUID-suffix
    // 1. Split by first '_' to separate date and rest
    let (_date, rest) = stem.split_once('_')?;

    // 2. Split by last '-' to separate UUID from suffix (e.g. "main")
    let (uuid, _suffix) = rest.rsplit_once('-')?;

    Some(uuid.to_string())
}

#[derive(Serialize, Deserialize)]
struct MemoriesHistory {
    #[serde(alias = "Saved Media")]
    saved_media: Vec<Media>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
enum MediaType {
    Image,
    Video,
}

#[derive(Serialize, Deserialize, Clone)]
struct Media {
    #[serde(alias = "Date", deserialize_with = "parse_date")]
    date: DateTime<Utc>,
    #[serde(alias = "Media Type")]
    media_type: MediaType,
    #[serde(alias = "Location", deserialize_with = "parse_coords")]
    coordinate: Coordinates,
    #[serde(alias = "Download Link", deserialize_with = "parse_id")]
    id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Coordinates {
    lat: f64,
    lon: f64,
}

fn parse_date<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    let dt = NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S UTC")
        .map_err(serde::de::Error::custom)?;
    Ok(DateTime::from_naive_utc_and_offset(dt, Utc))
}

fn parse_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    // 1. First, deserialize the field into a standard String
    let mut s: &str = Deserialize::deserialize(deserializer)?;

    let start_pattern = "&sid=";
    if let Some(idx) = s.find(start_pattern) {
        let start_index = idx + start_pattern.len();
        s = &s[start_index..];

        let end_pattern = "&mid";
        if let Some(end_index) = s.find(end_pattern) {
            let id = s[..end_index].to_string();
            return Ok(id);
        }
    }
    Err(serde::de::Error::custom(
        "Could not parse ID from Download Link",
    ))
}

fn parse_coords<'de, D>(deserializer: D) -> Result<Coordinates, D::Error>
where
    D: Deserializer<'de>,
{
    // 1. First, deserialize the field into a standard String
    let s: String = Deserialize::deserialize(deserializer)?;

    // 2. Process the string logic (finding the numbers after the colon)
    let parts: Vec<&str> = s
        .split(':')
        .next_back()
        .ok_or_else(|| serde::de::Error::custom("Missing colon in Location string"))?
        .split(',')
        .map(|p| p.trim())
        .collect();

    if parts.len() != 2 {
        return Err(serde::de::Error::custom(
            "Expected two comma-separated values",
        ));
    }

    // 3. Parse strings into floats
    let lat = parts[0].parse::<f64>().map_err(serde::de::Error::custom)?;
    let lon = parts[1].parse::<f64>().map_err(serde::de::Error::custom)?;

    Ok(Coordinates { lat, lon })
}

fn parse_memories_history_file(path: &Path) -> serde_json::Result<MemoriesHistory> {
    let data = std::fs::read(path).expect("File should be readable");
    serde_json::from_slice::<MemoriesHistory>(&data)
}
