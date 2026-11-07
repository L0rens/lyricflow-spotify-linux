use crate::storage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const FORMAT_VERSION: u32 = 1;
const MAX_CACHE_BYTES: u64 = 1_048_576;
const MAX_AGE_SECONDS: u64 = 30 * 24 * 60 * 60;

pub struct Lookup<'a> {
    pub key: &'a str,
    pub track: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub duration_ms: u64,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    version: u32,
    key: String,
    track: String,
    artist: String,
    album: String,
    duration_ms: u64,
    source: String,
    saved_at: u64,
    synced_lyrics: String,
}

fn filename(directory: &Path, key: &str) -> PathBuf {
    let hash: String = Sha256::digest(key.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    directory.join(format!("{hash}.json"))
}

fn epoch() -> Option<u64> {
    Some(SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs())
}

fn read_at(directory: &Path, lookup: &Lookup<'_>, now: u64) -> Option<String> {
    let mut bytes = Vec::new();
    fs::File::open(filename(directory, lookup.key))
        .ok()?
        .take(MAX_CACHE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return None;
    }
    let entry: Entry = serde_json::from_slice(&bytes).ok()?;
    if entry.version != FORMAT_VERSION
        || entry.source != "LRCLIB"
        || entry.key != lookup.key
        || entry.track != lookup.track
        || entry.artist != lookup.artist
        || entry.album != lookup.album
        || entry.duration_ms != lookup.duration_ms
        || entry.saved_at > now
        || now - entry.saved_at > MAX_AGE_SECONDS
    {
        return None;
    }
    Some(entry.synced_lyrics)
}

fn store_at(directory: &Path, lookup: &Lookup<'_>, source: &str, now: u64) -> anyhow::Result<()> {
    let entry = Entry {
        version: FORMAT_VERSION,
        key: lookup.key.into(),
        track: lookup.track.into(),
        artist: lookup.artist.into(),
        album: lookup.album.into(),
        duration_ms: lookup.duration_ms,
        source: "LRCLIB".into(),
        saved_at: now,
        synced_lyrics: source.into(),
    };
    let bytes = serde_json::to_vec(&entry)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_CACHE_BYTES,
        "Lyrics cache entry exceeds 1 MiB"
    );
    storage::write_atomic(&filename(directory, lookup.key), &bytes)
}

pub fn read(lookup: &Lookup<'_>) -> Option<String> {
    read_at(&storage::cache_dir().ok()?.join("lyrics"), lookup, epoch()?)
}

pub fn store(lookup: &Lookup<'_>, source: &str) {
    if let (Ok(directory), Some(now)) = (storage::cache_dir(), epoch()) {
        let _ = store_at(&directory.join("lyrics"), lookup, source, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_validates_identity_metadata_age_and_version() {
        let directory =
            std::env::temp_dir().join(format!("lyricflow-cache-{:016x}", rand::random::<u64>()));
        let lookup = Lookup {
            key: "spotify:track:test",
            track: "Song",
            artist: "Artist",
            album: "Album",
            duration_ms: 60_000,
        };
        store_at(&directory, &lookup, "[00:00]Local test lyric", 100).unwrap();
        assert!(
            read_at(&directory, &lookup, 101)
                .unwrap()
                .contains("Local test")
        );
        assert!(read_at(&directory, &lookup, 99).is_none());
        assert!(read_at(&directory, &lookup, 101 + MAX_AGE_SECONDS).is_none());
        let changed = Lookup {
            duration_ms: 61_000,
            ..lookup
        };
        assert!(read_at(&directory, &changed, 101).is_none());
        let path = filename(&directory, lookup.key);
        let mut data: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        data["version"] = 99.into();
        fs::write(&path, data.to_string()).unwrap();
        assert!(read_at(&directory, &lookup, 101).is_none());
        fs::write(&path, "broken cache").unwrap();
        assert!(read_at(&directory, &lookup, 101).is_none());
        assert_eq!(
            filename(&directory, "../../escape").parent().unwrap(),
            directory
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
