//! Synced lyrics from the public LRCLIB API (<https://lrclib.net>) plus an
//! LRC timestamp parser. Results are cached in memory and on disk
//! (`%LOCALAPPDATA%\WindowsDynamicNotch\lyrics`), negative results for a
//! limited time, so each track is requested at most once.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::{EventBus, ServiceEvent, storage};

const API: &str = "https://lrclib.net/api";
const USER_AGENT: &str = concat!("WindowsDynamicNotch/", env!("CARGO_PKG_VERSION"), " (desktop notch; lrclib client)");
const NEGATIVE_TTL: Duration = Duration::from_secs(12 * 3600);
const MEMORY_CACHE: usize = 48;

#[derive(Clone, Debug)]
pub struct LyricsRequest {
    pub key: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Duration,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LyricLine {
    pub time: Duration,
    pub text: String,
}

#[derive(Clone, Debug)]
pub enum LyricsResult {
    Synced(Arc<Vec<LyricLine>>),
    Plain(Arc<Vec<String>>),
    Instrumental,
    NotFound,
    /// Network or server failure; not cached.
    Unavailable,
}

#[derive(Clone)]
pub struct LyricsHandle(UnboundedSender<LyricsRequest>);

impl LyricsHandle {
    pub fn request(&self, request: LyricsRequest) {
        let _ = self.0.send(request);
    }
}

/// LRCLIB record (only the fields we use).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct Record {
    duration: Option<f64>,
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

/// On-disk cache entry.
#[derive(Debug, Deserialize, Serialize)]
struct CacheEntry {
    fetched: u64,
    record: Option<Record>,
}

pub fn spawn(rt: &Handle, bus: EventBus) -> LyricsHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    rt.spawn(run(bus, rx));
    LyricsHandle(tx)
}

async fn run(bus: EventBus, mut requests: UnboundedReceiver<LyricsRequest>) {
    let client = match reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(12))
        .connect_timeout(Duration::from_secs(6))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            log::error!("cannot build HTTP client: {err}");
            return;
        }
    };
    let cache_dir = storage::data_dir().join("lyrics");
    let _ = tokio::fs::create_dir_all(&cache_dir).await;
    let mut memory: HashMap<String, LyricsResult> = HashMap::new();

    while let Some(mut request) = requests.recv().await {
        // Only the most recent track matters.
        while let Ok(newer) = requests.try_recv() {
            request = newer;
        }
        if let Some(result) = memory.get(&request.key) {
            bus.send(ServiceEvent::Lyrics { key: request.key.clone(), result: result.clone() });
            continue;
        }
        let path = cache_dir.join(format!("{:016x}.json", hash(&request.key)));
        let result = match read_cache(&path).await {
            Some(record) => to_result(record),
            None => match fetch(&client, &request).await {
                Ok(record) => {
                    write_cache(&path, record.clone()).await;
                    to_result(record)
                }
                Err(err) => {
                    log::warn!("lyrics lookup failed for {:?}: {err}", request.title);
                    LyricsResult::Unavailable
                }
            },
        };
        if !matches!(result, LyricsResult::Unavailable) {
            if memory.len() >= MEMORY_CACHE {
                memory.clear();
            }
            memory.insert(request.key.clone(), result.clone());
        }
        bus.send(ServiceEvent::Lyrics { key: request.key, result });
    }
}

fn to_result(record: Option<Record>) -> LyricsResult {
    let Some(record) = record else { return LyricsResult::NotFound };
    if let Some(synced) = record.synced_lyrics.as_deref().filter(|s| !s.trim().is_empty()) {
        let lines = parse_lrc(synced);
        if !lines.is_empty() {
            return LyricsResult::Synced(Arc::new(lines));
        }
    }
    if record.instrumental {
        return LyricsResult::Instrumental;
    }
    match record.plain_lyrics.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(plain) => LyricsResult::Plain(Arc::new(plain.lines().map(|l| l.trim().to_string()).collect())),
        None => LyricsResult::NotFound,
    }
}

async fn read_cache(path: &PathBuf) -> Option<Option<Record>> {
    let bytes = tokio::fs::read(path).await.ok()?;
    let entry: CacheEntry = serde_json::from_slice(&bytes).ok()?;
    if entry.record.is_none() && now_secs().saturating_sub(entry.fetched) > NEGATIVE_TTL.as_secs() {
        return None;
    }
    Some(entry.record)
}

async fn write_cache(path: &PathBuf, record: Option<Record>) {
    let entry = CacheEntry { fetched: now_secs(), record };
    if let Ok(bytes) = serde_json::to_vec(&entry)
        && let Err(err) = tokio::fs::write(path, bytes).await
    {
        log::debug!("cannot write lyrics cache: {err}");
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `Ok(None)` means LRCLIB has no lyrics for the track.
async fn fetch(client: &reqwest::Client, request: &LyricsRequest) -> Result<Option<Record>, reqwest::Error> {
    let (artist, title) = normalize(&request.artist, &request.title);
    if title.is_empty() {
        return Ok(None);
    }
    let duration = request.duration.as_secs_f64();
    if !artist.is_empty() {
        let mut query = vec![("track_name", title.clone()), ("artist_name", artist.clone())];
        if !request.album.is_empty() {
            query.push(("album_name", request.album.clone()));
        }
        if duration > 1.0 {
            query.push(("duration", format!("{}", duration.round() as u64)));
        }
        let response = client.get(format!("{API}/get")).query(&query).send().await?;
        if response.status().is_success() {
            let record: Record = response.json().await?;
            if record.synced_lyrics.is_some() || record.plain_lyrics.is_some() || record.instrumental {
                return Ok(Some(record));
            }
        } else if response.status() != reqwest::StatusCode::NOT_FOUND {
            response.error_for_status()?;
        }
    }

    let mut query = vec![("track_name", title.clone())];
    if !artist.is_empty() {
        query.push(("artist_name", artist.clone()));
    }
    let response = client.get(format!("{API}/search")).query(&query).send().await?.error_for_status()?;
    let candidates: Vec<Record> = response.json().await?;
    Ok(best_match(candidates, duration))
}

/// Prefers synced lyrics with a matching duration.
fn best_match(candidates: Vec<Record>, duration: f64) -> Option<Record> {
    let close = |r: &Record| duration <= 1.0 || r.duration.is_none_or(|d| (d - duration).abs() <= 4.0);
    let has_synced = |r: &Record| r.synced_lyrics.as_deref().is_some_and(|s| !s.trim().is_empty());
    let mut fallback = None;
    for record in candidates {
        if has_synced(&record) && close(&record) {
            return Some(record);
        }
        if fallback.is_none() && close(&record) {
            fallback = Some(record);
        }
    }
    fallback
}

/// Strips video/decoration noise from titles reported by browsers and
/// splits "Artist - Title" when no artist is reported.
pub fn normalize(artist: &str, title: &str) -> (String, String) {
    let mut artist = artist.trim().to_string();
    let mut title = title.trim().to_string();
    if artist.is_empty()
        && let Some((a, t)) = title.split_once(" - ")
    {
        artist = a.trim().to_string();
        title = t.trim().to_string();
    }
    for suffix in [" - Topic", "VEVO", " - Official"] {
        if let Some(stripped) = artist.strip_suffix(suffix) {
            artist = stripped.trim().to_string();
        }
    }
    const NOISE: [&str; 12] = [
        "official", "video", "audio", "lyric", "lyrics", "visualizer", "remaster", "hd", "4k", "mv", "live", "clip",
    ];
    let mut cleaned = String::with_capacity(title.len());
    let mut rest = title.as_str();
    while let Some(open) = rest.find(['(', '[']) {
        let close_char = if rest[open..].starts_with('(') { ')' } else { ']' };
        let Some(close_rel) = rest[open..].find(close_char) else { break };
        let inner = rest[open + 1..open + close_rel].to_lowercase();
        cleaned.push_str(&rest[..open]);
        let noisy = inner
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| NOISE.iter().any(|noise| word == *noise || (noise.len() > 4 && word.starts_with(noise))));
        if !noisy && !inner.starts_with("feat") && !inner.starts_with("ft.") {
            cleaned.push_str(&rest[open..=open + close_rel]);
        }
        rest = &rest[open + close_rel + 1..];
    }
    cleaned.push_str(rest);
    let mut title = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    for marker in [" feat. ", " ft. ", " Feat. ", " Ft. "] {
        if let Some(index) = title.find(marker) {
            title.truncate(index);
        }
    }
    (artist, title.trim().trim_end_matches(['-', '|']).trim().to_string())
}

/// Parses LRC text: `[mm:ss.xx]` (several per line allowed), `[offset:±ms]`,
/// enhanced `<mm:ss.xx>` word tags are removed.
pub fn parse_lrc(text: &str) -> Vec<LyricLine> {
    let mut offset_ms: i64 = 0;
    let mut lines = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(end) = stripped.find(']') else { break };
            let tag = &stripped[..end];
            if let Some(time) = parse_timestamp(tag) {
                stamps.push(time);
            } else if let Some(value) = tag.strip_prefix("offset:") {
                offset_ms = value.trim().parse().unwrap_or(0);
            }
            rest = &stripped[end + 1..];
        }
        if stamps.is_empty() {
            continue;
        }
        let text = strip_word_tags(rest.trim());
        for time in stamps {
            lines.push((time, text.clone()));
        }
    }
    lines.sort_by_key(|(time, _)| *time);
    lines
        .into_iter()
        .map(|(time_ms, text)| LyricLine {
            // A positive offset makes lyrics appear sooner.
            time: Duration::from_millis((time_ms - offset_ms).max(0) as u64),
            text,
        })
        .collect()
}

fn parse_timestamp(tag: &str) -> Option<i64> {
    let (minutes, seconds) = tag.split_once(':')?;
    let minutes: i64 = minutes.trim().parse().ok()?;
    let seconds = seconds.trim().replacen(':', ".", 1);
    let seconds: f64 = seconds.parse().ok()?;
    if !(0.0..60.0).contains(&seconds) || minutes < 0 {
        return None;
    }
    Some(minutes * 60_000 + (seconds * 1000.0).round() as i64)
}

fn strip_word_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        match rest[open..].find('>') {
            Some(close) if parse_timestamp(&rest[open + 1..open + close]).is_some() => {
                out.push_str(&rest[..open]);
                rest = &rest[open + close + 1..];
            }
            _ => {
                out.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lrc() {
        let lines = parse_lrc("[ar:Someone]\n[00:12.50]first\n[00:05.00][01:00.00]chorus\n[offset:500]\n[00:20.123]<00:20.12>wor <00:20.50>ds");
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].text, "chorus");
        assert_eq!(lines[0].time, Duration::from_millis(4500));
        assert_eq!(lines[1].text, "first");
        assert_eq!(lines[2].text, "wor ds");
        assert_eq!(lines[3].time, Duration::from_millis(59_500));
    }

    #[test]
    fn normalizes_titles() {
        assert_eq!(normalize("", "Lumen Coast - Paper Sky (Official Video)"), ("Lumen Coast".into(), "Paper Sky".into()));
        assert_eq!(normalize("ArtistVEVO", "Song [Lyrics] feat. X"), ("Artist".into(), "Song".into()));
        assert_eq!(normalize("A", "Track (Acoustic)"), ("A".into(), "Track (Acoustic)".into()));
    }
}
