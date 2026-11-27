//! The history of submitted lines, optionally kept in a file.
//!
//! The file backend is the only code of this crate that does input/output (`std::fs`).

use std::collections::{HashSet, VecDeque};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Default number of entries kept.
pub(crate) const DEFAULT_CAPACITY: usize = 1000;
/// Most bytes read from the end of the history file when it is loaded: a larger file keeps
/// only its newest lines (and is rewritten). Far more than 1000 entries of a 4 KiB line.
pub(crate) const MAX_LOAD_BYTES: u64 = 16 * 1024 * 1024;
/// Attempts at creating a temporary file with a new name before giving up.
const TEMPORARY_ATTEMPTS: u32 = 8;

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
    /// that a crash loses nothing and the appends of two instances never corrupt each other).
    ///
    /// Format: UTF-8, one entry per line. Lines that are not valid UTF-8 or that contain a
    /// control character are skipped; duplicates are removed (the latest is kept), then
    /// the last `capacity` entries are kept. Only the last 16 MiB of the file are
    /// read. When the file holds more than twice the capacity, or more than 16 MiB,
    /// it is rewritten: through a new temporary file in the same directory (created with a
    /// name nobody else uses, never through an existing file or symbolic link) and a rename.
    /// Just before the rename, the lines another instance appended since the load are read
    /// again and kept; a line appended at that very instant can still be lost.
    ///
    /// A missing file is not an error; its directory is created if needed. A path that is
    /// not a regular file (a directory, a FIFO, a device) is an error, checked on the opened
    /// file itself at each open (load, append, rewrite), so that a file replaced by a FIFO
    /// meanwhile is refused instead of blocking; on Unix, the files are opened without
    /// blocking and without following a symbolic link. On Unix, a new directory gets mode
    /// `0o700`, and the file is private to the user: created with mode `0o600`, and an
    /// existing file readable by others is changed to `0o600`.
    ///
    /// A path that is a symbolic link is resolved once, here: the file it points to is the
    /// history (its mode is the one made private), and the rewrite replaces that file, so
    /// the link stays. A link to a missing file is an error. The file is meant for the
    /// history only: a file that holds anything else loses its content beyond the last
    /// `capacity` distinct lines at the first rewrite.
    ///
    /// The second value is the error, if any. After an error of the load, the history works
    /// in memory only. After an error of the rewrite (the file was read, but the temporary
    /// file could not be written or renamed: read-only directory, full disk…), the lines are
    /// still appended to the file, which keeps its size until a later rewrite succeeds.
    /// [`History::file`] tells the two apart.
    pub fn with_file(path: impl Into<PathBuf>, capacity: usize) -> (Self, Option<io::Error>) {
        let mut history = Self::new(capacity);
        let path = match resolve_link(path.into()) {
            Ok(path) => path,
            Err(error) => return (history, Some(error)),
        };
        match load(&path, capacity, MAX_LOAD_BYTES) {
            Ok(loaded) => {
                history.entries = loaded.entries.into();
                let mut error = None;
                if loaded.lines > capacity.saturating_mul(2) || loaded.truncated {
                    match rewrite(&path, history.iter(), loaded.len, capacity) {
                        Ok(entries) => history.entries = entries.into(),
                        // Appending needs only the file, which was just read.
                        Err(rewrite_error) => error = Some(rewrite_error),
                    }
                }
                history.file = Some(path);
                (history, error)
            }
            Err(error) => (history, Some(error)),
        }
    }

    /// The file each recorded line is appended to, `None` when the history is in memory
    /// only (no file, an error of the load or of an append).
    #[must_use]
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
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

/// What [`load`] read.
#[derive(Debug, Default)]
struct Loaded {
    /// The valid entries, deduplicated, at most `capacity`, oldest first.
    entries: Vec<String>,
    /// Number of non-empty lines read.
    lines: usize,
    /// Length of the file when it was read: what another instance appends comes after.
    len: u64,
    /// Whether the start of the file was not read (longer than the bound).
    truncated: bool,
}

/// `path` itself, or the file it points to if it is a symbolic link (see
/// [`History::with_file`]).
fn resolve_link(path: PathBuf) -> io::Result<PathBuf> {
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::canonicalize(&path),
        _ => Ok(path),
    }
}

/// Open `path` with `options`, and check that it is a regular file on the open file itself
/// (no gap between the check and the use). On Unix, without blocking (opening a FIFO would
/// wait for its other end) and without following a symbolic link (the path was resolved by
/// [`resolve_link`]: a link there now was put in its place since).
fn open_regular(options: &mut OpenOptions, path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if file.metadata()?.is_file() {
        Ok(file)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ))
    }
}

/// Read the history file: at most its last `max_bytes` bytes (the first, partial line of
/// that part is dropped). A missing file is empty; its directory is created. Anything but a
/// regular file is an error (see [`open_regular`]).
fn load(path: &Path, capacity: usize, max_bytes: u64) -> io::Result<Loaded> {
    let mut file = match open_regular(OpenOptions::new().read(true), path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                create_dir(dir)?;
            }
            return Ok(Loaded::default());
        }
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    make_private(&file, &metadata)?;
    let len = metadata.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    // `take`: the file may grow while it is read.
    file.take(max_bytes).read_to_end(&mut bytes)?;
    let mut text: &[u8] = &bytes;
    if start > 0 {
        // The first line is probably cut: start after it.
        text = text.splitn(2, |&b| b == b'\n').nth(1).unwrap_or_default();
    }
    let (entries, lines) = newest_entries(text, capacity, Vec::new());
    Ok(Loaded {
        entries,
        lines,
        len: start + u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        truncated: start > 0,
    })
}

/// The valid lines of `text` (deduplicated, the latest kept) after the entries `older`,
/// then the newest `capacity` of them, oldest first; and the number of non-empty lines of
/// `text`. Walks the lines from the newest, copying only the kept ones.
fn newest_entries(text: &[u8], capacity: usize, older: Vec<String>) -> (Vec<String>, usize) {
    let lines: Vec<&[u8]> = text
        .split(|&b| b == b'\n')
        .map(|raw| raw.strip_suffix(b"\r").unwrap_or(raw))
        .filter(|raw| !raw.is_empty())
        .collect();
    let valid = lines.iter().rev().filter_map(|raw| {
        std::str::from_utf8(raw)
            .ok()
            .filter(|line| !line.chars().any(char::is_control) && !line.trim().is_empty())
    });
    let mut seen = HashSet::new();
    let mut kept: Vec<String> = valid
        .map(str::to_owned)
        .chain(older.into_iter().rev())
        .filter(|line| seen.insert(line.clone()))
        .take(capacity)
        .collect();
    kept.reverse();
    (kept, lines.len())
}

/// On Unix, make an existing history file private to the user (mode `0o600`) if others can
/// read or write it. Through the open file: never another file put at its path.
#[cfg_attr(
    not(unix),
    expect(clippy::unnecessary_wraps, reason = "the Unix version can fail")
)]
fn make_private(file: &File, metadata: &fs::Metadata) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
    }
    #[cfg(not(unix))]
    let _ = (file, metadata);
    Ok(())
}

/// Append one entry to the file, in a single write.
fn append(path: &Path, line: &str) -> io::Result<()> {
    let mut file = open_regular(open_options().append(true).create(true), path)?;
    file.write_all(format!("{line}\n").as_bytes())
}

/// A temporary name next to `path` that no other instance or attempt uses: the process id,
/// the time and a counter of this process.
fn temporary_name(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp-{}-{nanos}-{count}", std::process::id()));
    path.with_file_name(name)
}

/// Create a new file at the first name of `names` that does not exist: an existing file or
/// symbolic link is never opened (`create_new`).
fn create_temporary(names: impl Iterator<Item = PathBuf>) -> io::Result<(PathBuf, File)> {
    let mut last = io::Error::new(io::ErrorKind::AlreadyExists, "no temporary name");
    for name in names {
        match open_options().write(true).create_new(true).open(&name) {
            Ok(file) => return Ok((name, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => last = error,
            Err(error) => return Err(error),
        }
    }
    Err(last)
}

/// Replace the file with `entries` plus the lines appended after byte `loaded_len` by
/// another instance (at most `capacity` entries in all), through a new temporary file in the
/// same directory, so that a crash never leaves a truncated history. Return the entries
/// written.
fn rewrite<'a>(
    path: &Path,
    entries: impl Iterator<Item = &'a str>,
    loaded_len: u64,
    capacity: usize,
) -> io::Result<Vec<String>> {
    let names = (0..TEMPORARY_ATTEMPTS).map(|_| temporary_name(path));
    rewrite_with(path, entries, loaded_len, capacity, names)
}

/// [`rewrite`] with the temporary names to try.
fn rewrite_with<'a>(
    path: &Path,
    entries: impl Iterator<Item = &'a str>,
    loaded_len: u64,
    capacity: usize,
    names: impl Iterator<Item = PathBuf>,
) -> io::Result<Vec<String>> {
    let (temporary, mut file) = create_temporary(names)?;
    let result = (|| {
        let appended = read_after(path, loaded_len)?;
        let (merged, _) = newest_entries(&appended, capacity, entries.map(str::to_owned).collect());
        let mut text = String::new();
        for entry in &merged {
            text.push_str(entry);
            text.push('\n');
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(merged)
    })();
    if result.is_err() {
        // Only the temporary file this call created.
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// The bytes of the file after `offset` (at most [`MAX_LOAD_BYTES`]): what other instances
/// appended since it was read. Empty if the file got shorter (rewritten meanwhile): reading
/// from past its end gives nothing.
fn read_after(path: &Path, offset: u64) -> io::Result<Vec<u8>> {
    let mut file = open_regular(OpenOptions::new().read(true), path)?;
    let mut bytes = Vec::new();
    file.seek(SeekFrom::Start(offset))?;
    file.take(MAX_LOAD_BYTES).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Options for the files of the history: private to the user on Unix.
fn open_options() -> OpenOptions {
    #[cfg_attr(not(unix), expect(unused_mut, reason = "only Unix sets a mode"))]
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
    #[cfg(unix)]
    #[test]
    fn an_existing_loose_file_becomes_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let path = dir.0.join("history");
        fs::write(&path, "a\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let (history, error) = History::with_file(&path, 10);
        assert!(error.is_none(), "{error:?}");
        assert_eq!(entries(&history), ["a"]);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_a_temporary_name_is_never_followed() {
        let dir = TempDir::new();
        let path = dir.0.join("history");
        let target = dir.0.join("target");
        fs::write(&target, "keep\n").unwrap();
        // The name of the previous versions, `history.tmp-<pid>`, then a taken name: both
        // are skipped, the third one is used.
        let old = dir.0.join(format!("history.tmp-{}", std::process::id()));
        std::os::unix::fs::symlink(&target, &old).unwrap();
        let taken = dir.0.join("taken");
        std::os::unix::fs::symlink(&target, &taken).unwrap();
        fs::write(&path, "a\n".repeat(30)).unwrap();
        let _ = History::with_file(&path, 4);
        let free = dir.0.join("free");
        let names = [taken.clone(), free.clone()].into_iter();
        let written = rewrite_with(&path, ["x", "y"].into_iter(), 0, 4, names).unwrap();
        assert_eq!(written, ["x", "y", "a"]);
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep\n");
        assert_eq!(fs::read_to_string(&path).unwrap(), "x\ny\na\n");
        assert!(!free.exists(), "the temporary file is renamed");
        // When every name is taken: an error, and nothing removed.
        let error = rewrite_with(&path, ["z"].into_iter(), 0, 4, [taken.clone()].into_iter());
        assert_eq!(error.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert!(fs::symlink_metadata(&taken).is_ok());
    }

    #[test]
    fn a_line_appended_between_load_and_rewrite_is_kept() {
        let dir = TempDir::new();
        let path = dir.0.join("history");
        fs::write(&path, "a\nb\n".repeat(10)).unwrap();
        let loaded = load(&path, 4, MAX_LOAD_BYTES).unwrap();
        // Another instance appends while this one prepares its rewrite.
        let (mut other, _) = History::with_file(&path, 100);
        other.add("from-the-other");
        let written = rewrite(
            &path,
            loaded.entries.iter().map(String::as_str),
            loaded.len,
            4,
        )
        .unwrap();
        assert_eq!(written, ["a", "b", "from-the-other"]);
        assert_eq!(fs::read_to_string(&path).unwrap(), "a\nb\nfrom-the-other\n");
    }

    #[test]
    fn a_file_larger_than_the_bound_keeps_its_newest_entries() {
        let dir = TempDir::new();
        let path = dir.0.join("history");
        let mut text = String::new();
        for i in 0..100 {
            // Writing to a `String` cannot fail.
            let _ = std::fmt::Write::write_fmt(&mut text, format_args!("command {i:03}\n"));
        }
        fs::write(&path, &text).unwrap();
        // 60 bytes from the end: the first, cut line is dropped.
        let loaded = load(&path, 10, 60).unwrap();
        assert!(loaded.truncated);
        assert_eq!(
            loaded.entries,
            ["command 096", "command 097", "command 098", "command 099"]
        );
        assert_eq!(loaded.len, u64::try_from(text.len()).unwrap());
        let small = load(&path, 2, 60).unwrap();
        assert_eq!(small.entries, ["command 098", "command 099"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_device_is_refused_at_once() {
        let (history, error) = History::with_file("/dev/zero", 10);
        assert_eq!(error.unwrap().kind(), io::ErrorKind::InvalidInput);
        assert!(history.is_empty());
    }
    #[test]
    fn capacity_and_emptiness() {
        let mut history = History::new(7);
        assert_eq!(history.capacity(), 7);
        assert!(history.is_empty());
        history.add("a");
        assert!(!history.is_empty());
    }

    #[test]
    fn a_file_of_exactly_twice_the_capacity_is_not_rewritten() {
        let dir = TempDir::new();
        let path = dir.0.join("history");
        let text = "a\nb\n".repeat(4);
        fs::write(&path, &text).unwrap();
        // 8 lines, capacity 4: not more than twice.
        let _ = History::with_file(&path, 4);
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn the_load_bound_is_16_mib() {
        assert_eq!(MAX_LOAD_BYTES, 16 * 1024 * 1024);
        // 2 MiB: read whole.
        let dir = TempDir::new();
        let path = dir.0.join("history");
        let mut text = "x".repeat(2 * 1024 * 1024);
        text.push_str("\nfirst\n");
        let text = format!("oldest\n{text}");
        fs::write(&path, &text).unwrap();
        let loaded = load(&path, 10, MAX_LOAD_BYTES).unwrap();
        assert!(!loaded.truncated);
        assert_eq!(loaded.entries.first().map(String::as_str), Some("oldest"));
    }

    #[test]
    fn reading_after_the_end_gives_nothing() {
        let dir = TempDir::new();
        let path = dir.0.join("history");
        fs::write(&path, "abc\n").unwrap();
        assert_eq!(read_after(&path, 4).unwrap(), b"");
        assert_eq!(read_after(&path, 100).unwrap(), b"");
        assert_eq!(read_after(&path, 1).unwrap(), b"bc\n");
    }

    #[test]
    fn only_a_taken_name_makes_the_rewrite_try_another_one() {
        let dir = TempDir::new();
        let path = dir.0.join("history");
        fs::write(&path, "a\n").unwrap();
        // A name in a missing directory: that error is returned, the next name is not tried.
        let names = [dir.0.join("missing/tmp"), dir.0.join("free")].into_iter();
        let error = rewrite_with(&path, ["x"].into_iter(), 0, 4, names).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!dir.0.join("free").exists());
        assert_eq!(fs::read_to_string(&path).unwrap(), "a\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_file_private_to_the_user_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let path = dir.0.join("history");
        fs::write(&path, "a\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        let _ = History::with_file(&path, 10);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o400
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_directory_is_an_error_not_an_empty_history() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let locked = dir.0.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("history"), "a\n").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let result = load(&locked.join("history"), 10, MAX_LOAD_BYTES);
        // As root, permissions stop nothing: the file can be read, and loading succeeds.
        let root = fs::read(locked.join("history")).is_ok();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        if root {
            assert!(result.is_ok());
        } else {
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_rewrite_keeps_appending_to_the_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let locked = dir.0.join("locked");
        fs::create_dir(&locked).unwrap();
        let path = locked.join("history");
        // 6 lines, capacity 2: more than twice, so rewritten; but no temporary file can be
        // created in a read-only directory, while the file itself stays writable.
        fs::write(&path, "a\nb\nc\nd\ne\nf\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();
        let (mut history, error) = History::with_file(&path, 2);
        // As root, permissions stop nothing: the rewrite succeeds.
        let root = error.is_none();
        history.add("g");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(history.file(), Some(path.as_path()));
        assert_eq!(history.iter().collect::<Vec<_>>(), ["f", "g"]);
        let text = fs::read_to_string(&path).unwrap();
        if root {
            assert_eq!(text, "e\nf\ng\n");
        } else {
            assert_eq!(error.unwrap().kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(text, "a\nb\nc\nd\ne\nf\ng\n");
        }
    }

    /// A FIFO at `path` (through `mkfifo`: the standard library cannot make one).
    #[cfg(unix)]
    fn make_fifo(path: &Path) {
        let status = std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .expect("cannot run mkfifo");
        assert!(status.success(), "mkfifo failed");
    }

    /// Run `f` on another thread, and fail if it does not return within 5 seconds (a FIFO
    /// opened in blocking mode would wait forever).
    #[cfg(unix)]
    fn without_blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("blocked on the history file")
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_refused_at_once() {
        let dir = TempDir::new();
        let path = dir.0.join("history");
        make_fifo(&path);
        let (file, kind) = without_blocking(move || {
            let (history, error) = History::with_file(&path, 10);
            (history.file().map(Path::to_owned), error.map(|e| e.kind()))
        });
        assert_eq!(file, None);
        assert_eq!(kind, Some(io::ErrorKind::InvalidInput));
    }

    #[cfg(unix)]
    #[test]
    fn a_file_replaced_by_a_fifo_after_the_load_is_refused_at_once() {
        let dir = TempDir::new();
        let path = dir.0.join("history");
        fs::write(&path, "a\n").unwrap();
        let (mut history, error) = History::with_file(&path, 10);
        assert!(error.is_none());
        fs::remove_file(&path).unwrap();
        make_fifo(&path);
        let (history, recorded) = without_blocking(move || {
            let recorded = history.add("b");
            (history, recorded)
        });
        let mut history = history;
        // Kept in memory; the file is given up, and the error reported once.
        assert!(recorded);
        assert_eq!(history.iter().collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(history.file(), None);
        assert!(history.take_error().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_is_resolved_and_kept_by_the_rewrite() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let target = dir.0.join("target");
        let link = dir.0.join("link");
        // 6 lines, capacity 2: rewritten at the load.
        fs::write(&target, "a\nb\nc\nd\ne\nf\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let (mut history, error) = History::with_file(&link, 2);
        assert!(error.is_none(), "{error:?}");
        history.add("g");
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&link).unwrap(), "e\nf\ng\n");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            history.file(),
            Some(fs::canonicalize(&target).unwrap().as_path())
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_missing_file_is_an_error() {
        let dir = TempDir::new();
        let link = dir.0.join("link");
        std::os::unix::fs::symlink(dir.0.join("missing"), &link).unwrap();
        let (history, error) = History::with_file(&link, 10);
        assert_eq!(error.unwrap().kind(), io::ErrorKind::NotFound);
        assert_eq!(history.file(), None);
        assert!(!dir.0.join("missing").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_put_at_the_path_after_the_load_is_not_followed() {
        let dir = TempDir::new();
        let path = dir.0.join("history");
        let other = dir.0.join("other");
        fs::write(&path, "a\n").unwrap();
        fs::write(&other, "untouched\n").unwrap();
        let (mut history, _) = History::with_file(&path, 10);
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&other, &path).unwrap();
        history.add("b");
        assert_eq!(fs::read_to_string(&other).unwrap(), "untouched\n");
        assert_eq!(history.file(), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_load_leaves_no_file() {
        let (history, error) = History::with_file("/dev/zero", 10);
        assert!(error.is_some());
        assert_eq!(history.file(), None);
    }
}
