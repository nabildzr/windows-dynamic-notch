//! Local JSON persistence in `%LOCALAPPDATA%\WindowsDynamicNotch`.
//!
//! The UI thread only serializes; reading and writing happens on the tokio
//! runtime. Writes are debounced per document and atomic (temp file +
//! rename) so a crash never leaves a truncated file behind.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{self, UnboundedSender};

use super::{EventBus, ServiceEvent};

const APP_DIR: &str = "WindowsDynamicNotch";
const DEBOUNCE: Duration = Duration::from_millis(400);

/// Persisted documents, one JSON file each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Document {
    Settings,
    Todo,
    Water,
    Pomodoro,
    Shelf,
    Clipboard,
}

impl Document {
    pub const ALL: [Document; 6] = [
        Document::Settings,
        Document::Todo,
        Document::Water,
        Document::Pomodoro,
        Document::Shelf,
        Document::Clipboard,
    ];

    pub fn file_name(self) -> &'static str {
        match self {
            Document::Settings => "settings.json",
            Document::Todo => "todo.json",
            Document::Water => "water.json",
            Document::Pomodoro => "pomodoro.json",
            Document::Shelf => "shelf.json",
            Document::Clipboard => "clipboard.json",
        }
    }
}

/// Raw bytes of every document found on disk at startup.
#[derive(Debug, Default)]
pub struct LoadedDocuments {
    pub files: HashMap<Document, Vec<u8>>,
}

impl LoadedDocuments {
    /// Deserializes a document, falling back to the default on absence or
    /// corruption (the corrupt file is kept as `*.bak` for inspection).
    pub fn parse<T: DeserializeOwned + Default>(&self, document: Document) -> T {
        let Some(bytes) = self.files.get(&document) else { return T::default() };
        match serde_json::from_slice(bytes) {
            Ok(value) => value,
            Err(err) => {
                log::warn!("{} is invalid ({err}); starting fresh", document.file_name());
                let path = data_dir().join(document.file_name());
                let _ = std::fs::copy(&path, path.with_extension("json.bak"));
                T::default()
            }
        }
    }
}

/// `%LOCALAPPDATA%\WindowsDynamicNotch`, created on demand.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join(APP_DIR);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[derive(Clone)]
pub struct StorageHandle {
    tx: UnboundedSender<(Document, Vec<u8>)>,
}

impl StorageHandle {
    /// Queues an asynchronous, debounced save.
    pub fn save<T: Serialize>(&self, document: Document, value: &T) {
        match serde_json::to_vec_pretty(value) {
            Ok(bytes) => {
                let _ = self.tx.send((document, bytes));
            }
            Err(err) => log::error!("cannot serialize {}: {err}", document.file_name()),
        }
    }

    /// Writes immediately on the calling thread. Only used while shutting
    /// down, when the runtime may not get another chance to run.
    pub fn save_now<T: Serialize>(&self, document: Document, value: &T) {
        match serde_json::to_vec_pretty(value) {
            Ok(bytes) => {
                if let Err(err) = write_atomic_blocking(&data_dir().join(document.file_name()), &bytes) {
                    log::error!("cannot write {}: {err}", document.file_name());
                }
            }
            Err(err) => log::error!("cannot serialize {}: {err}", document.file_name()),
        }
    }
}

pub fn spawn(rt: &Handle, bus: EventBus) -> StorageHandle {
    let (tx, mut rx) = mpsc::unbounded_channel::<(Document, Vec<u8>)>();

    rt.spawn(async move {
        let dir = data_dir();
        let mut loaded = LoadedDocuments::default();
        for document in Document::ALL {
            if let Ok(bytes) = tokio::fs::read(dir.join(document.file_name())).await {
                loaded.files.insert(document, bytes);
            }
        }
        bus.send(ServiceEvent::Loaded(Box::new(loaded)));
    });

    rt.spawn(async move {
        let dir = data_dir();
        let mut pending: HashMap<Document, Vec<u8>> = HashMap::new();
        loop {
            if pending.is_empty() {
                match rx.recv().await {
                    Some((document, bytes)) => {
                        pending.insert(document, bytes);
                    }
                    None => break,
                }
            }
            // Collect further edits until the debounce window is quiet.
            match tokio::time::timeout(DEBOUNCE, rx.recv()).await {
                Ok(Some((document, bytes))) => {
                    pending.insert(document, bytes);
                    continue;
                }
                Ok(None) => {
                    flush(&dir, &mut pending).await;
                    break;
                }
                Err(_) => flush(&dir, &mut pending).await,
            }
        }
    });

    StorageHandle { tx }
}

async fn flush(dir: &Path, pending: &mut HashMap<Document, Vec<u8>>) {
    for (document, bytes) in pending.drain() {
        let path = dir.join(document.file_name());
        let tmp = path.with_extension("json.tmp");
        let result = async {
            tokio::fs::write(&tmp, &bytes).await?;
            tokio::fs::rename(&tmp, &path).await
        }
        .await;
        if let Err(err) = result {
            log::error!("cannot write {}: {err}", path.display());
        }
    }
}

fn write_atomic_blocking(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}
