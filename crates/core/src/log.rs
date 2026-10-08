//! A plain text journal of what the app did, for the day something fails
//! without telling anyone: an update that never arrives, a sync server that
//! exits, a tool an agent keeps getting an error from.

use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const MAX_BYTES: u64 = 1024 * 1024;
const FILE_NAME: &str = "rovibe.log";

struct Journal {
    path: PathBuf,
    file: File,
}

static JOURNAL: OnceLock<Mutex<Journal>> = OnceLock::new();

/// Opens the journal. A file that has grown past its limit is set aside
/// first: one previous journal is kept, older ones are not.
pub fn init(data_dir: &Path) {
    let path = data_dir.join(FILE_NAME);
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_BYTES) {
        let _ = std::fs::rename(&path, data_dir.join("rovibe.log.1"));
    }
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = JOURNAL.set(Mutex::new(Journal { path, file }));
    }
}

#[cfg(windows)]
fn timestamp() -> String {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut now = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut now) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond
    )
}

#[cfg(not(windows))]
fn timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    format!("t+{seconds}")
}

fn write(level: &str, message: &str) {
    let Some(journal) = JOURNAL.get() else {
        return;
    };
    // One entry per line, whatever the message contains.
    let line = format!("{} {level:<5} {}\n", timestamp(), message.replace('\n', " | "));
    let _ = journal.lock().unwrap().file.write_all(line.as_bytes());
}

pub fn info(message: impl AsRef<str>) {
    write("INFO", message.as_ref());
}

pub fn warn(message: impl AsRef<str>) {
    write("WARN", message.as_ref());
}

pub fn path() -> Option<PathBuf> {
    JOURNAL.get().map(|journal| journal.lock().unwrap().path.clone())
}

/// The last lines of the journal, oldest first.
pub fn tail(lines: usize) -> Vec<String> {
    let Some(path) = path() else {
        return Vec::new();
    };
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };

    // Enough to hold the lines asked for without reading a whole megabyte.
    let length = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(length.saturating_sub(256 * 1024)));
    let mut bytes = Vec::new();
    let _ = file.read_to_end(&mut bytes);

    let text = String::from_utf8_lossy(&bytes);
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..]
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
}
