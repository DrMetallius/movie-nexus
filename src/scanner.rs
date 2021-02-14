use std::{
    collections::HashMap,
    time::Duration,
    path::{PathBuf, Path, Component},
    io,
    fs,
};
use serde::{Deserialize, Serialize, Serializer, ser};
use std::collections::HashSet;

const EXTENSION_MP4: &str = "mp4";
const EXTENSION_TOML: &str = "toml";
const EXTENSION_SUBTITLES: &str = "vtt";

const DEFAULT_LANGUAGE: &str = "en";

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum CatalogueItem {
    #[serde(rename = "directory")]
    Directory {
        #[serde(rename = "title")] name: String,
        #[serde(rename = "contents")] items: Vec<CatalogueItem>,
    },
    #[serde(rename = "file")]
    Video {
        path: RelativizedPath,
        title: String,
        subtitle: Option<String>,
        #[serde(serialize_with = "serialize_duration")]
        duration: Duration,
        #[serde(rename = "text-tracks", skip_serializing_if = "HashMap::is_empty")]
        text_tracks: HashMap<String, RelativizedPath>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        thumbnails: Vec<PathBuf>,
    },
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct RelativizedPath {
    pub path: PathBuf,
    pub relative_path: PathBuf,
}

impl RelativizedPath {
    fn new(root_path: impl AsRef<Path>, path: impl AsRef<Path>) -> RelativizedPath {
        RelativizedPath {
            path: PathBuf::from(path.as_ref()),
            relative_path: path.as_ref().strip_prefix(root_path).unwrap().to_path_buf(),
        }
    }
}

impl Serialize for RelativizedPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
        let universal_path = self.relative_path.components()
            .map(|component| {
                match component {
                    Component::Normal(str) => Ok(str.to_str().unwrap()),
                    _ => {
                        let message = format!("Path {} does not consist only of normal components", self.relative_path.to_string_lossy());
                        Err(ser::Error::custom(message))
                    }
                }
            })
            .collect::<Result<Vec<&str>, S::Error>>()?
            .join("/");

        serializer.serialize_str(&universal_path)
    }
}

fn serialize_duration<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64(duration.as_millis() as u64)
}

pub fn scan_directory(root_path: &Path, path: &Path) -> Result<Vec<CatalogueItem>, io::Error> {
    let mut items: Vec<CatalogueItem> = Vec::new();
    for child_path in fs::read_dir(path)? {
        let entry = child_path?;
        let file_name = entry.file_name().into_string().unwrap();
        let file_type = entry.file_type()?;

        let path = entry.path();

        if file_type.is_dir() {
            items.push(CatalogueItem::Directory {
                name: file_name,
                items: scan_directory(root_path, &path)?,
            })
        } else if file_type.is_file() {
            if let Some(extension) = path.extension() {
                if extension != EXTENSION_MP4 { continue; }

                let mut toml_path = path.clone();
                toml_path.set_extension(EXTENSION_TOML);
                if !toml_path.is_file() { continue; }

                let config = match toml::from_str::<Config>(&fs::read_to_string(toml_path)?) {
                    Ok(file) => file,
                    Err(_) => continue,
                };

                let duration = iso8601::duration(&config.duration);
                let duration = if let Ok(iso8601::Duration::YMDHMS { hour, minute, second, millisecond, .. }) = duration {
                    let milliseconds_total = hour as u64 * 60 * 60 * 1000 + minute as u64 * 60 * 1000 + second as u64 * 1000 + millisecond as u64;
                    Duration::from_millis(milliseconds_total)
                } else {
                    continue;
                };

                let mut text_tracks: HashMap<String, RelativizedPath> = HashMap::new();

                let mut subtitle_path = path.clone();
                subtitle_path.set_extension(EXTENSION_SUBTITLES);
                if subtitle_path.is_file() {
                    let language = config.text_track_language.unwrap_or(DEFAULT_LANGUAGE.into());
                    text_tracks.insert(language, RelativizedPath::new(root_path, subtitle_path));
                }

                items.push(CatalogueItem::Video {
                    path: RelativizedPath::new(root_path, path),
                    title: config.title,
                    subtitle: config.subtitle,
                    duration,
                    text_tracks,
                    thumbnails: Vec::new(),
                })
            }
        }
    }

    Ok(items)
}

#[derive(Deserialize)]
struct Config {
    title: String,
    subtitle: Option<String>,
    duration: String,
    #[serde(rename = "text-track-language")]
    text_track_language: Option<String>,
}

pub fn extract_served_files(catalogue: &Vec<CatalogueItem>) -> HashSet<RelativizedPath> {
    catalogue.iter()
        .flat_map(|item| {
            match item {
                CatalogueItem::Video { path, .. } => Some(path.clone()).into_iter().collect(),
                CatalogueItem::Directory { items, .. } => extract_served_files(items)
            }
        })
        .collect()
}