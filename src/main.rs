use clap::Parser;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserializer;
use serde::{Deserialize, Serialize};

fn main() {
    let args = Args::parse();
    let memories_data = parse_memories_history_file(&args.memories_history_json_path).unwrap();

    for media in memories_data.saved_media {
        let media_path = match media.media_type {
            MediaType::Image => media.id + ".jpg",
            MediaType::Video => media.id + ".mp4",
        };
    }
}

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "./json/memories_history.json")]
    memories_history_json_path: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct MemoriesHistory {
    #[serde(alias = "Saved Media")]
    saved_media: Vec<Media>,
}

#[derive(Debug, Serialize, Deserialize)]
enum MediaType {
    Image,
    Video,
}

#[derive(Serialize, Deserialize)]
struct Media {
    #[serde(alias = "Date")]
    date: String,
    #[serde(alias = "Media Type")]
    media_type: MediaType,
    #[serde(alias = "Location", deserialize_with = "parse_coords")]
    coordinate: Coordinates,
    #[serde(alias = "Download Link", deserialize_with = "parse_id")]
    id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Coordinates {
    lat: f64,
    lon: f64,
}

fn parse_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    // 1. First, deserialize the field into a standard String
    let mut s: &str = Deserialize::deserialize(deserializer)?;

    let start_pattern = "&sid=";
    let start_index = s.find(start_pattern).unwrap() + start_pattern.len();

    s = &s[start_index..];

    let end_pattern = "&mid";
    let end_index = s.find(end_pattern).unwrap();

    let id = s[..end_index].to_string();

    // 2. Process the string logic (finding the numbers after the colon)
    Ok(id)
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

    // Parse the string of data into a Person object. This is exactly the
    // same function as the one that produced serde_json::Value above, but
    // now we are asking it for a Person as output.
    serde_json::from_slice::<MemoriesHistory>(&data)
}
