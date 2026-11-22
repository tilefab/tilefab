//! The history of submitted lines, optionally kept in a file.
//!
//! The file backend is the only code of this crate that does input/output (`std::fs`).

use std::collections::{HashSet, VecDeque};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Default number of entries kept.
pub(crate) const DEFAULT_CAPACITY: usize = 1000;

/// Submitted lines, oldest first. Every line appears once, at its latest position.
///
/// With [`History::with_file`], each recorded line is also appended to a file, and the
/// file is loaded at startup. File errors never stop the history: it keeps working in
/// memory, and the error is reported once.
#[derive(Debug)]
pub struct History {
    entries: VecDeque<String>,
    capacity: usize,
    /// The file each recorded line is appended to; `None` after a write error.
    file: Option<PathBuf>,
    /// A write error not reported yet.
    error: Option<io::Error>,
}

impl Default for History {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl History {
    /// An empty history that keeps at most `capacity` entries.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity,
            file: None,
            error: None,
        }
    }

    /// A history kept in the file at `path`: the file is loaded now, and each recorded line
    /// is appended to it at once (one write per line on a file opened in append mode, so
    /// that a crash loses nothing and two instances do not corrupt each other).
    ///
    /// Format: UTF-8, one entry per line. Lines that are not valid UTF-8 or that contain a
    /// control character are skipped; duplicates are removed (the latest is kept), then
    /// the last `capacity` entries are kept. When the file holds more than twice the
    /// capacity, it is rewritten (through a temporary file in the same directory and a
    /// rename). A missing file is not an error; its directory is created if needed. On
    /// Unix, a new directory gets mode `0o700` and a new file mode `0o600`.
    ///
    /// The second value is the load error, if any: the history then works in memory only.
    pub fn with_file(path: impl Into<PathBuf>, capacity: usize) -> (Self, Option<io::Error>) {
        let path = path.into();
        let mut history = Self::new(capacity);
        match load(&path, capacity) {
            Ok((entries, lines)) => {
                history.entries = entries.into();
                if lines > capacity.saturating_mul(2)
                    && let Err(error) = rewrite(&path, history.iter())
                {
                    return (history, Some(error));
                }
                history.file = Some(path);
                (history, None)
            }
            Err(error) => (history, Some(error)),
        }
    }

    /// The last error writing the history file, once: after it, the history is kept in
    /// memory only.
    pub fn take_error(&mut self) -> Option<io::Error> {
        self.error.take()
    }

    /// Maximum number of entries kept.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if there is no entry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry at `index`, 0 being the oldest.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(String::as_str)
    }

    /// The entries, oldest first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &str> + ExactSizeIterator {
        self.entries.iter().map(String::as_str)
    }

    /// Add `line` as the newest entry; an older identical entry is removed. Empty and
    /// whitespace-only lines, and lines with a control character, are not recorded.
    /// Return true if the line was recorded.
    pub fn add(&mut self, line: &str) -> bool {
        if line.trim().is_empty() || line.chars().any(char::is_control) || self.capacity == 0 {
            return false;
        }
        self.entries.retain(|entry| entry != line);
        self.entries.push_back(line.to_string());
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
        if let Some(path) = &self.file
            && let Err(error) = append(path, line)
        {
            self.file = None;
            self.error = Some(error);
        }
        true
    }
}

/// Read the history file: its valid entries (deduplicated, at most `capacity`) and its
/// number of non-empty lines. A missing file is empty; its directory is created.
fn load(path: &Path, capacity: usize) -> io::Result<(Vec<String>, usize)> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                create_dir(dir)?;
            }
            return Ok((Vec::new(), 0));
        }
        Err(error) => return Err(error),
    };
    let mut lines = 0;
    let mut entries = Vec::new();
    for raw in bytes.split(|&b| b == b'\n') {
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        if raw.is_empty() {
            continue;
        }
        lines += 1;
        if let Ok(line) = std::str::from_utf8(raw)
            && !line.chars().any(char::is_control)
            && !line.trim().is_empty()
        {
            entries.push(line.to_string());
        }
    }
    // Keep the latest occurrence of each line, then the newest `capacity` entries.
    let mut seen = HashSet::new();
    let mut kept: Vec<String> = entries
        .into_iter()
        .rev()
        .filter(|line| seen.insert(line.clone()))
        .take(capacity)
        .collect();
    kept.reverse();
    Ok((kept, lines))
}

/// Append one entry to the file, in a single write.
fn append(path: &Path, line: &str) -> io::Result<()> {
    let mut file = open_options().append(true).create(true).open(path)?;
    file.write_all(format!("{line}\n").as_bytes())
}

/// Replace the file with `entries`, through a temporary file in the same directory, so
/// that a crash never leaves a truncated history.
fn rewrite<'a>(path: &Path, entries: impl Iterator<Item = &'a str>) -> io::Result<()> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp-{}", std::process::id()));
    let temporary = path.with_file_name(name);
    let mut text = String::new();
    for entry in entries {
        text.push_str(entry);
        text.push('\n');
    }
    let result = (|| {
        let mut file: File = open_options()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Options for the files of the history: private to the user on Unix.
fn open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// Create `dir` and its parents; private to the user on Unix.
fn create_dir(dir: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A new, unique directory under the system temporary directory, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("tilefab-input-history-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("cannot create the test directory");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn entries(history: &History) -> Vec<&str> {
        history.iter().collect()
    }

    #[test]
    fn duplicates_keep_only_the_latest_position() {
        let mut history = History::new(10);
        for line in ["a", "b", "a", "c", "b"] {
            assert!(history.add(line));
        }
        assert_eq!(entries(&history), ["a", "c", "b"]);
    }

    #[test]
    fn empty_whitespace_and_control_lines_are_not_recorded() {
        let mut history = History::new(10);
        assert!(!history.add(""));
        assert!(!history.add("   "));
        assert!(!history.add("a\u{7}"));
        assert!(history.is_empty());
    }

    #[test]
    fn the_oldest_entries_are_dropped_beyond_the_capacity() {
        let mut history = History::new(2);
        for line in ["a", "b", "c"] {
            history.add(line);
        }
        assert_eq!(entries(&history), ["b", "c"]);
        assert_eq!((history.get(0), history.get(2)), (Some("b"), None));
        let mut none = History::new(0);
        assert!(!none.add("a"));
    }

    #[test]
    fn a_missing_file_starts_empty_and_creates_its_directory() {
        let dir = TempDir::new();
        let path = dir.0.join("state/tilefab/history");
        let (mut history, error) = History::with_file(&path, 10);
        assert!(error.is_none(), "{error:?}");
        assert!(history.is_empty());
        assert!(path.parent().unwrap().is_dir());
        history.add("ls");
        history.add("pwd");
        assert_eq!(fs::read_to_string(&path).unwrap(), "ls\npwd\n");
        assert!(history.take_error().is_none());
    }

    #[test]
    fn loading_skips_invalid_lines_and_keeps_the_latest_duplicates() {
        let dir = TempDir::new();
        let path = dir.0.join("history");
        fs::write(&path, b"a\nb\n\xff\xfe\nbad\x07line\n\na\r\nc\n   \n").unwrap();
        let (history, error) = History::with_file(&path, 10);
        assert!(error.is_none());
        assert_eq!(history.iter().collect::<Vec<_>>(), ["b", "a", "c"]);
        let (small, _) = History::with_file(&path, 2);
        assert_eq!(small.iter().collect::<Vec<_>>(), ["a", "c"]);
    }

    #[test]
    fn a_file_twice_too_long_is_compacted_on_load() {
        let dir = TempDir::new();
        let path = dir.0.join("history");
        let lines = "cmd0\ncmd1\ncmd2\n".repeat(3) + "cmd0\n";
        fs::write(&path, &lines).unwrap();
        // 10 lines, capacity 4: more than twice the capacity.
        let (history, error) = History::with_file(&path, 4);
        assert!(error.is_none());
        assert_eq!(history.iter().collect::<Vec<_>>(), ["cmd1", "cmd2", "cmd0"]);
        assert_eq!(fs::read_to_string(&path).unwrap(), "cmd1\ncmd2\ncmd0\n");
        // Not rewritten below the limit.
        fs::write(&path, "a\na\n").unwrap();
        let _ = History::with_file(&path, 4);
        assert_eq!(fs::read_to_string(&path).unwrap(), "a\na\n");
        // No temporary file left.
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn new_files_and_directories_are_private_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let path = dir.0.join("private/history");
        let (mut history, _) = History::with_file(&path, 10);
        history.add("secret");
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
    }

    #[test]
    fn io_errors_keep_the_history_in_memory_and_are_reported_once() {
        let dir = TempDir::new();
        // A directory where the file should be: loading fails.
        let (mut history, error) = History::with_file(&dir.0, 10);
        assert!(error.is_some());
        assert!(history.add("ls"));
        assert_eq!(history.get(0), Some("ls"));
        // A file that becomes unwritable: the first write fails, once.
        let path = dir.0.join("history");
        let (mut history, error) = History::with_file(&path, 10);
        assert!(error.is_none());
        fs::create_dir(&path).unwrap();
        assert!(history.add("a"));
        assert!(history.take_error().is_some());
        assert!(history.add("b"));
        assert!(history.take_error().is_none(), "reported once");
        assert_eq!(history.iter().collect::<Vec<_>>(), ["a", "b"]);
    }
}
