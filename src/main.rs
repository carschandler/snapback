use clap::{Parser, ValueEnum};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use chrono::{DateTime, NaiveDateTime, Utc};
use glob::glob;
use rayon::prelude::*;
use serde::Deserializer;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, ValueEnum)]
enum CaptionMode {
    /// Skip caption overlays entirely
    Ignore,
    /// Create a _captioned copy while preserving the original
    Copy,
    /// Apply caption overlay directly to the original file
    Overwrite,
}

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "./json/memories_history.json")]
    memories_history_json_path: PathBuf,

    /// Directory containing zip files to unpack
    #[arg(short, long, default_value = ".")]
    zip_dir: PathBuf,

    /// Number of concurrent exiftool/ffmpeg processes
    #[arg(short, long, default_value_t = 1)]
    processes: usize,

    /// Directory to move processed media files into
    #[arg(short, long, default_value = "./processed_media")]
    output_dir: PathBuf,

    /// Directory name prefix to glob for media files (e.g. "memories" matches "memories*/**/*.jpg")
    #[arg(long, default_value = "memories")]
    media_prefix: String,

    /// How to handle caption overlays: ignore, copy (creates _captioned version), or overwrite
    #[arg(short, long, value_enum, default_value_t = CaptionMode::Copy)]
    captions: CaptionMode,
}

fn main() {
    let args = Args::parse();

    // Set up parallel processing
    rayon::ThreadPoolBuilder::new()
        .num_threads(args.processes)
        .build_global()
        .unwrap();

    // Unzip logic
    let zip_dir = &args.zip_dir;
    let zip_pattern = zip_dir.join("*.zip");
    let zip_pattern_str = zip_pattern.to_str().expect("Invalid zip path pattern");

    println!("Looking for zip files in: {}", zip_pattern_str);

    for entry in glob(zip_pattern_str).expect("Failed to read glob pattern for zips") {
        match entry {
            Ok(path) => {
                println!("Unzipping {:?}", path);
                let status = Command::new("unzip")
                    .arg("-o") // Overwrite existing files without prompting
                    .arg(&path)
                    .arg("-d")
                    .arg(".") // Extract to current directory
                    .status();

                match status {
                    Ok(s) => {
                        if !s.success() {
                            eprintln!("Unzip failed for {:?}", path);
                        } else {
                            println!("Successfully unzipped {:?}", path);
                        }
                    }
                    Err(e) => eprintln!("Failed to execute unzip for {:?}: {}", path, e),
                }
            }
            Err(e) => eprintln!("Glob error: {:?}", e),
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

    let caption_mode = &args.captions;

    paths.par_iter().for_each(|path| {
        let file_name_str = path
            .file_name()
            .unwrap_or_default()
            .to_str()
            .unwrap_or("?")
            .to_string();
        let mut did_exif = false;
        let mut did_caption = false;

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
                                eprintln!("ExifTool failed for {:?}", path);
                            }
                        }
                        Err(e) => eprintln!("Failed to execute ExifTool for {:?}: {}", path, e),
                    }
                }
            }
        }

        // 2. Apply caption overlay (after EXIF so metadata is already set)
        if !matches!(caption_mode, CaptionMode::Ignore) {
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
                            // convert to real PNG for ffmpeg
                            let converted_overlay = path.with_file_name(format!(
                                "{}_overlay.png",
                                path.file_stem().unwrap().to_str().unwrap()
                            ));
                            let overlay_to_use = match fs::read(&overlay_path) {
                                Ok(bytes) => match image::load_from_memory(&bytes) {
                                    Ok(img) => match img.save(&converted_overlay) {
                                        Ok(()) => &converted_overlay,
                                        Err(e) => {
                                            eprintln!("Failed to save converted overlay: {}", e);
                                            &overlay_path
                                        }
                                    },
                                    Err(e) => {
                                        eprintln!(
                                            "Failed to decode overlay {:?}: {}",
                                            overlay_path, e
                                        );
                                        &overlay_path
                                    }
                                },
                                Err(e) => {
                                    eprintln!(
                                        "Failed to read overlay file {:?}: {}",
                                        overlay_path, e
                                    );
                                    &overlay_path
                                }
                            };

                            let ext = path.extension().unwrap_or_default().to_str().unwrap_or("");
                            let stem = path.file_stem().unwrap().to_str().unwrap();
                            let (input_path, final_output) = match caption_mode {
                                CaptionMode::Copy => {
                                    let captioned =
                                        path.with_file_name(format!("{}_captioned.{}", stem, ext));
                                    if let Err(e) = fs::copy(path, &captioned) {
                                        eprintln!(
                                            "Failed to copy {:?} for captioning: {}",
                                            path, e
                                        );
                                        let _ = fs::remove_file(&converted_overlay);
                                        return;
                                    }
                                    (captioned.clone(), captioned)
                                }
                                CaptionMode::Overwrite => (path.to_path_buf(), path.to_path_buf()),
                                CaptionMode::Ignore => unreachable!(),
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
                                            eprintln!(
                                                "Failed to finalize captioned file {:?}: {}",
                                                final_output, e
                                            );
                                        } else {
                                            did_caption = true;
                                        }
                                    } else {
                                        eprintln!("FFmpeg failed for caption on {:?}", path);
                                        let _ = fs::remove_file(&temp_output);
                                        if matches!(caption_mode, CaptionMode::Copy) {
                                            let _ = fs::remove_file(&final_output);
                                        }
                                    }
                                }
                                Err(e) => eprintln!("Failed to run FFmpeg for {:?}: {}", path, e),
                            }
                        }
                    }
                }
            }
        }

        // Log once per file
        match (did_exif, did_caption) {
            (true, true) => println!("Added EXIF data and caption to {}", file_name_str),
            (true, false) => println!("Added EXIF data to {}", file_name_str),
            (false, true) => println!("Added caption to {}", file_name_str),
            (false, false) => {}
        }
    });

    // 3. Move processed media files to output directory
    let output_dir = &args.output_dir;
    if let Err(e) = fs::create_dir_all(output_dir) {
        eprintln!("Failed to create output directory {:?}: {}", output_dir, e);
        return;
    }

    let mut moved = 0usize;
    for path in &paths {
        let file_name = match path.file_name() {
            Some(name) => name,
            None => continue,
        };

        let dest = output_dir.join(file_name);
        match fs::rename(path, &dest) {
            Ok(()) => moved += 1,
            Err(e) => eprintln!("Failed to move {:?} to {:?}: {}", path, dest, e),
        }

        // Also move the _captioned version if it exists (copy mode)
        if matches!(args.captions, CaptionMode::Copy) {
            let ext = path.extension().unwrap_or_default().to_str().unwrap_or("");
            let stem = path.file_stem().unwrap().to_str().unwrap();
            let captioned = path.with_file_name(format!("{}_captioned.{}", stem, ext));
            if captioned.exists() {
                let captioned_dest = output_dir.join(captioned.file_name().unwrap());
                match fs::rename(&captioned, &captioned_dest) {
                    Ok(()) => moved += 1,
                    Err(e) => eprintln!(
                        "Failed to move {:?} to {:?}: {}",
                        captioned, captioned_dest, e
                    ),
                }
            }
        }
    }

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
