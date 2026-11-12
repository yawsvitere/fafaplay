use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use lofty::{picture::PictureType, prelude::*, tag::ItemKey};
use walkdir::WalkDir;

pub const EXTENSIONS: [&str; 3] = ["mp3", "flac", "wav"];

pub struct CoverData {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub track_no: Option<u32>,
    pub disc: Option<u32>,
    pub duration: Duration,
}

pub struct Album {
    pub title: String,
    pub year: Option<u32>,
    pub cover: Option<Arc<CoverData>>,
    pub tracks: Vec<Track>,
}

pub struct Artist {
    pub name: String,
    pub albums: Vec<Album>,
}

#[derive(Default)]
pub struct Library {
    pub artists: Vec<Artist>,
}

pub enum ScanMsg {
    Progress(usize),
    Done(Library),
}

/// Буква для группировки в сайдбаре. Всё, что не буква, уходит в '#'.
pub fn letter_of(name: &str) -> char {
    match name.chars().find(|c| c.is_alphanumeric()) {
        Some(c) if c.is_alphabetic() => c.to_uppercase().next().unwrap_or('#'),
        _ => '#',
    }
}

struct Parsed {
    track: Track,
    artist: String,
    album: String,
    year: Option<u32>,
    picture: Option<Vec<u8>>,
}

fn parse(path: &Path) -> Parsed {
    let title = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".into());

    let mut p = Parsed {
        track: Track {
            path: path.to_path_buf(),
            title,
            track_no: None,
            disc: None,
            duration: Duration::ZERO,
        },
        artist: String::new(),
        album: String::new(),
        year: None,
        picture: None,
    };

    if let Ok(tagged) = lofty::read_from_path(path) {
        p.track.duration = tagged.properties().duration();

        if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
            if let Some(t) = tag.title() {
                let t = t.trim().to_string();
                if !t.is_empty() {
                    p.track.title = t;
                }
            }

            let album_artist = tag
                .get_string(&ItemKey::AlbumArtist)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let artist = tag
                .artist()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            p.artist = album_artist.or(artist).unwrap_or_default();

            if let Some(a) = tag.album() {
                p.album = a.trim().to_string();
            }

            p.track.track_no = tag.track();
            p.track.disc = tag.disk();
            p.year = tag.year();

            let pic = tag
                .pictures()
                .iter()
                .find(|x| x.pic_type() == PictureType::CoverFront)
                .or_else(|| tag.pictures().first());
            p.picture = pic.map(|x| x.data().to_vec());
        }
    }

    if p.artist.is_empty() {
        p.artist = "Unknown Artist".into();
    }
    if p.album.is_empty() {
        p.album = path
            .parent()
            .and_then(|d| d.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Unknown Album".into());
    }
    p
}

fn decode_cover(bytes: &[u8]) -> Option<Arc<CoverData>> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = img.thumbnail(640, 640).to_rgba8();
    let (w, h) = img.dimensions();
    Some(Arc::new(CoverData {
        width: w as usize,
        height: h as usize,
        rgba: img.into_raw(),
    }))
}

/// Ищет cover.jpg / folder.png и т.п. рядом с треками.
fn folder_cover(dir: &Path) -> Option<Arc<CoverData>> {
    const NAMES: [&str; 6] = ["cover", "folder", "front", "album", "albumart", "artwork"];
    const EXTS: [&str; 3] = ["jpg", "jpeg", "png"];

    for entry in fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        let (Some(stem), Some(ext)) = (p.file_stem(), p.extension()) else {
            continue;
        };
        let stem = stem.to_string_lossy().to_lowercase();
        let ext = ext.to_string_lossy().to_lowercase();
        if NAMES.contains(&stem.as_str()) && EXTS.contains(&ext.as_str()) {
            if let Some(c) = fs::read(&p).ok().and_then(|b| decode_cover(&b)) {
                return Some(c);
            }
        }
    }
    None
}

/// Рекурсивно сканирует папку. Вызывается из фонового потока.
pub fn scan(root: &Path, mut progress: impl FnMut(usize)) -> Library {
    let mut artists: BTreeMap<String, Artist> = BTreeMap::new();
    let mut n = 0usize;

    for entry in WalkDir::new(root).follow_links(true).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let supported = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .is_some_and(|e| EXTENSIONS.contains(&e.as_str()));
        if !supported {
            continue;
        }

        let parsed = parse(path);
        n += 1;
        if n % 5 == 0 {
            progress(n);
        }

        let artist = artists
            .entry(parsed.artist.to_lowercase())
            .or_insert_with(|| Artist {
                name: parsed.artist.clone(),
                albums: Vec::new(),
            });

        let key = parsed.album.to_lowercase();
        let idx = match artist
            .albums
            .iter()
            .position(|a| a.title.to_lowercase() == key)
        {
            Some(i) => i,
            None => {
                artist.albums.push(Album {
                    title: parsed.album.clone(),
                    year: None,
                    cover: None,
                    tracks: Vec::new(),
                });
                artist.albums.len() - 1
            }
        };

        let album = &mut artist.albums[idx];
        if album.year.is_none() {
            album.year = parsed.year.filter(|y| *y > 0);
        }
        if album.cover.is_none() {
            if let Some(bytes) = &parsed.picture {
                album.cover = decode_cover(bytes);
            }
        }
        album.tracks.push(parsed.track);
    }
    progress(n);

    let mut artists: Vec<Artist> = artists.into_values().collect();
    for artist in &mut artists {
        for album in &mut artist.albums {
            album.tracks.sort_by(|a, b| {
                (a.disc.unwrap_or(1), a.track_no.unwrap_or(u32::MAX), &a.path).cmp(&(
                    b.disc.unwrap_or(1),
                    b.track_no.unwrap_or(u32::MAX),
                    &b.path,
                ))
            });
            if album.cover.is_none() {
                if let Some(dir) = album.tracks.first().and_then(|t| t.path.parent()) {
                    album.cover = folder_cover(dir);
                }
            }
        }
        artist.albums.sort_by(|a, b| {
            (a.year.unwrap_or(u32::MAX), a.title.to_lowercase())
                .cmp(&(b.year.unwrap_or(u32::MAX), b.title.to_lowercase()))
        });
    }

    Library { artists }
}
