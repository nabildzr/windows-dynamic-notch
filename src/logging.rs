//! Minimal file logger: `%LOCALAPPDATA%\WindowsDynamicNotch\notch.log`.
//!
//! Our own crate logs at `info` (or `debug` with `NOTCH_LOG=debug`), every
//! other crate at `warn`. The file is truncated when it grows past 1 MiB.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;

use log::{Level, LevelFilter, Log, Metadata, Record};

const MAX_LOG_BYTES: u64 = 1024 * 1024;
const OWN_TARGET: &str = "windows_dynamic_notch";

struct NotchLogger {
    file: Option<Mutex<File>>,
    own_level: Level,
}

impl Log for NotchLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        if metadata.target().starts_with(OWN_TARGET) {
            metadata.level() <= self.own_level
        } else {
            metadata.level() <= Level::Warn
        }
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:<5} [{}] {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            record.level(),
            record.target(),
            record.args()
        );
        if cfg!(debug_assertions) {
            eprint!("{line}");
        }
        if let Some(file) = &self.file
            && let Ok(mut file) = file.lock()
        {
            let _ = file.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Some(file) = &self.file
            && let Ok(mut file) = file.lock()
        {
            let _ = file.flush();
        }
    }
}

pub fn init() {
    let path = crate::services::storage::data_dir().join("notch.log");
    let too_big = std::fs::metadata(&path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false);
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(!too_big)
        .truncate(too_big)
        .open(&path)
        .ok()
        .map(Mutex::new);
    let own_level = match std::env::var("NOTCH_LOG").as_deref() {
        Ok("debug") => Level::Debug,
        Ok("trace") => Level::Trace,
        _ => Level::Info,
    };
    let logger = NotchLogger { file, own_level };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(LevelFilter::Trace);
    }
    std::panic::set_hook(Box::new(|info| {
        log::error!("panic: {info}");
        log::logger().flush();
    }));
}
