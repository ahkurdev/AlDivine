//! Project Aldivine Diagnostic File Logging & Panic Diagnostics
//!
//! Provides lightweight, dependency-free file loggers, timestamp generators,
//! panic hooks that dump stack information into `logs/crash.log`, and log readers
//! for inspecting runtime bugs.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Standard Gregorian date computation from days since 1970-01-01 (Unix Epoch).
/// Based on Howard Hinnant's public domain civil calendar algorithm.
fn days_to_ymd(days: u64) -> (u32, u32, u32) {
    let z = days as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u32, m, d)
}

/// Returns a human-readable ISO-like UTC timestamp: `YYYY-MM-DD HH:MM:SS.mmm UTC`.
pub fn format_timestamp(now: SystemTime) -> String {
    let duration = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let total_secs = duration.as_secs();
    let millis = duration.subsec_millis();

    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hours = (total_secs / 3600) % 24;
    let days = total_secs / 86400;

    let (year, month, day) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} UTC", year, month, day, hours, mins, secs, millis)
}

/// Severity level of diagnostic messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
            LogLevel::Fatal => "FATAL",
        }
    }
}

/// Thread-safe diagnostic file logger.
#[derive(Clone)]
pub struct DiagnosticLogger {
    file_path: PathBuf,
    writer: Arc<Mutex<Option<File>>>,
}

impl DiagnosticLogger {
    /// Initialize a logger writing to `logs_dir/filename`.
    pub fn new(logs_dir: impl AsRef<Path>, filename: &str) -> Self {
        let dir = logs_dir.as_ref();
        let _ = fs::create_dir_all(dir);
        let path = dir.join(filename);

        let file = OpenOptions::new().create(true).append(true).open(&path).ok();

        Self { file_path: path, writer: Arc::new(Mutex::new(file)) }
    }

    /// Append a log message with level, timestamp, and optional target module.
    pub fn log(&self, level: LogLevel, target: &str, message: &str) {
        let timestamp = format_timestamp(SystemTime::now());
        let line = format!("[{}] [{}] [{}] {}\n", timestamp, level.as_str(), target, message);

        // Also print to stderr for errors/warnings, stdout for info
        if level >= LogLevel::Error {
            eprint!("{}", line);
        } else {
            print!("{}", line);
        }

        if let Ok(mut guard) = self.writer.lock() {
            if let Some(ref mut file) = *guard {
                let _ = file.write_all(line.as_bytes());
                let _ = file.flush();
            }
        }
    }

    pub fn info(&self, target: &str, message: &str) {
        self.log(LogLevel::Info, target, message);
    }

    pub fn warn(&self, target: &str, message: &str) {
        self.log(LogLevel::Warn, target, message);
    }

    pub fn error(&self, target: &str, message: &str) {
        self.log(LogLevel::Error, target, message);
    }

    pub fn path(&self) -> &Path {
        &self.file_path
    }
}

/// Installs a global panic hook that writes panic payloads, locations, and
/// system metadata to `logs/<app_name>_crash.log` and `logs/crash.log`.
pub fn install_crash_handler(logs_dir: impl AsRef<Path>, app_name: &str) {
    let dir = logs_dir.as_ref().to_path_buf();
    let app = app_name.to_string();

    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |panic_info| {
        let timestamp = format_timestamp(SystemTime::now());
        let _ = fs::create_dir_all(&dir);

        let location = panic_info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());

        let payload = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Box<Any> panic payload".to_string()
        };

        let dump = format!(
            "=====================================================\n\
             ALDIVINE CRASH REPORT\n\
             Timestamp: {}\n\
             Application: {}\n\
             Location: {}\n\
             Panic Message: {}\n\
             OS / Arch: {} / {}\n\
             =====================================================\n",
            timestamp,
            app,
            location,
            payload,
            std::env::consts::OS,
            std::env::consts::ARCH,
        );

        eprintln!("{}", dump);

        // Write to app-specific crash log and general crash log
        for fname in &[format!("{}_crash.log", app), "crash.log".to_string()] {
            let crash_file = dir.join(fname);
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(crash_file) {
                let _ = f.write_all(dump.as_bytes());
                let _ = f.flush();
            }
        }

        default_hook(panic_info);
    }));
}

/// Diagnostic summary of a log file.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LogSummary {
    pub total_lines: usize,
    pub info_count: usize,
    pub warn_count: usize,
    pub error_count: usize,
    pub fatal_count: usize,
    pub recent_errors: Vec<String>,
}

/// Analyze a log file and summarize its health status.
pub fn inspect_log_file(path: impl AsRef<Path>) -> std::io::Result<LogSummary> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let mut summary = LogSummary::default();

    for line_res in reader.lines() {
        let line = line_res?;
        summary.total_lines += 1;

        if line.contains("[INFO]") {
            summary.info_count += 1;
        } else if line.contains("[WARN]") {
            summary.warn_count += 1;
        } else if line.contains("[ERROR]") {
            summary.error_count += 1;
            if summary.recent_errors.len() < 10 {
                summary.recent_errors.push(line);
            }
        } else if line.contains("[FATAL]") {
            summary.fatal_count += 1;
            if summary.recent_errors.len() < 10 {
                summary.recent_errors.push(line);
            }
        }
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_timestamp_produces_iso_like_string() {
        let ts = format_timestamp(SystemTime::now());
        assert!(ts.contains("UTC"));
        assert_eq!(ts.len(), 27); // "YYYY-MM-DD HH:MM:SS.mmm UTC"
    }

    #[test]
    fn diagnostic_logger_writes_and_flushes() {
        let temp_dir = std::env::temp_dir().join("ald_test_logs");
        let logger = DiagnosticLogger::new(&temp_dir, "test.log");

        logger.info("server", "server booted successfully");
        logger.warn("network", "slow client ping detected");
        logger.error("database", "deadlock on transaction");

        let summary = inspect_log_file(logger.path()).expect("read log");
        assert_eq!(summary.total_lines, 3);
        assert_eq!(summary.info_count, 1);
        assert_eq!(summary.warn_count, 1);
        assert_eq!(summary.error_count, 1);
        assert_eq!(summary.recent_errors.len(), 1);
        assert!(summary.recent_errors[0].contains("deadlock on transaction"));

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
