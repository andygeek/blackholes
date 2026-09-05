use anyhow::{Context, Result, bail};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::ffi::OsStringExt as _,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};
use uuid::Uuid;

pub const MAXIMUM_EDITABLE_FILE_BYTES: usize = 8 * 1024 * 1024;
pub const MAXIMUM_EDITABLE_FILE_LINES: usize = 50_000;
pub const MAXIMUM_INDEXED_REPOSITORY_FILES: usize = 50_000;
pub const MAXIMUM_DIFF_ROWS: usize = 20_000;

#[derive(Clone, Debug)]
pub struct IndexedRepositoryFile {
    pub path: PathBuf,
    pub relative_path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileEntryKind {
    Directory,
    File,
    Symlink,
}

impl FileEntryKind {
    pub fn is_directory(self) -> bool {
        self == Self::Directory
    }
}

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub kind: FileEntryKind,
    pub hidden: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepositoryChangeKind {
    Added,
    Deleted,
    Modified,
    Renamed,
    Untracked,
    Conflicted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryChange {
    pub path: PathBuf,
    pub relative_path: String,
    pub previous_relative_path: Option<String>,
    pub kind: RepositoryChangeKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepositoryDiffLineKind {
    Context,
    Changed,
    Added,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepositoryDiffRow {
    Hunk {
        old_start: usize,
        new_start: usize,
        header: String,
    },
    Line {
        old_number: Option<usize>,
        new_number: Option<usize>,
        old_text: String,
        new_text: String,
        kind: RepositoryDiffLineKind,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryFileDiff {
    pub original: Option<Arc<str>>,
    pub modified: Option<Arc<str>>,
    pub rows: Arc<[RepositoryDiffRow]>,
    pub binary: bool,
    pub truncated: bool,
}

/// Returns every working-tree change relative to HEAD, including staged, unstaged, renamed, and
/// untracked files. Porcelain v1 with NUL delimiters keeps paths with whitespace unambiguous.
pub fn repository_changes(root: &Path) -> Result<Vec<RepositoryChange>> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("Unable to resolve repository {}", root.display()))?;
    if !root.is_dir() {
        bail!("The selected repository is not a directory");
    }

    let output = Command::new("git")
        .current_dir(&root)
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
        ])
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("Unable to inspect Git changes in {}", root.display()))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("Git could not inspect {}: {message}", root.display());
    }

    let fields = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    let mut changes = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let field = fields[index];
        index += 1;
        if field.len() < 4 || field.get(2) != Some(&b' ') {
            continue;
        }

        let x = field[0] as char;
        let y = field[1] as char;
        let relative_path = String::from_utf8_lossy(&field[3..]).into_owned();
        let renamed_or_copied = matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C');
        let previous_relative_path = if renamed_or_copied && index < fields.len() {
            let previous = String::from_utf8_lossy(fields[index]).into_owned();
            index += 1;
            Some(previous)
        } else {
            None
        };
        let kind = repository_change_kind(x, y);
        changes.push(RepositoryChange {
            path: root.join(&relative_path),
            relative_path,
            previous_relative_path,
            kind,
        });
    }
    changes.sort_by_cached_key(|change| change.relative_path.to_ascii_lowercase());
    Ok(changes)
}

/// Builds a bounded, side-by-side representation of a file's working-tree diff against HEAD.
pub fn repository_file_diff(root: &Path, change: &RepositoryChange) -> Result<RepositoryFileDiff> {
    let mut diff = repository_file_patch(root, change)?;
    if diff.binary { return Ok(diff); }
    // Monaco needs complete documents, not concatenated hunks. Keep the bounded
    // patch fallback for files that exceed editor limits or cannot be decoded.
    let contents = (|| -> Result<(String, String)> {
        let original = if matches!(change.kind, RepositoryChangeKind::Added | RepositoryChangeKind::Untracked) {
            String::new()
        } else {
            let path = change.previous_relative_path.as_deref().unwrap_or(&change.relative_path);
            let mut child = Command::new("git").current_dir(root)
                .args(["show", &format!("HEAD:{path}")])
                .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
            let mut bytes = Vec::new();
            let read = child.stdout.take().context("Missing Git output")?
                .take(MAXIMUM_EDITABLE_FILE_BYTES as u64 + 1).read_to_end(&mut bytes);
            if read.is_err() || bytes.len() > MAXIMUM_EDITABLE_FILE_BYTES {
                let _ = child.kill();
                let _ = child.wait();
                bail!("Original file exceeds editor limits");
            }
            if !child.wait()?.success() || bytes.contains(&0)
                || bytes.iter().filter(|byte| **byte == b'\n').count() + 1 > MAXIMUM_EDITABLE_FILE_LINES {
                bail!("Original file cannot be displayed in the editor");
            }
            String::from_utf8(bytes)?
        };
        let modified = if change.kind == RepositoryChangeKind::Deleted {
            String::new()
        } else { read_text_file(root, &change.path)? };
        Ok((original, modified))
    })();
    if let Ok((original, modified)) = contents {
        diff.original = Some(original.into());
        diff.modified = Some(modified.into());
    }
    Ok(diff)
}

fn repository_file_patch(root: &Path, change: &RepositoryChange) -> Result<RepositoryFileDiff> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("Unable to resolve repository {}", root.display()))?;
    if !root.is_dir() {
        bail!("The selected repository is not a directory");
    }

    let mut child = Command::new("git")
        .current_dir(&root)
        .args([
            "diff",
            "--no-ext-diff",
            "--no-color",
            "--find-renames",
            "--unified=3",
            "HEAD",
            "--",
        ])
        .arg(&change.relative_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("Unable to compare {}", change.relative_path))?;
    let maximum = MAXIMUM_EDITABLE_FILE_BYTES * 2;
    let mut bytes = Vec::new();
    let read = child.stdout.take().context("Missing Git diff output")?
        .take(maximum as u64 + 1).read_to_end(&mut bytes);
    let limited = bytes.len() > maximum;
    if limited || read.is_err() { let _ = child.kill(); }
    let status = child.wait()?;
    read?;
    if (status.success() || limited) && !bytes.is_empty() {
        bytes.truncate(maximum);
        let mut diff = parse_repository_patch(&String::from_utf8_lossy(&bytes));
        diff.truncated |= limited;
        return Ok(diff);
    }

    if matches!(
        change.kind,
        RepositoryChangeKind::Added | RepositoryChangeKind::Untracked
    ) {
        return added_file_diff(&root, &change.path);
    }

    if !status.success() {
        bail!("Git could not compare {} ({status})", change.relative_path);
    }

    Ok(RepositoryFileDiff {
        original: None, modified: None,
        rows: Vec::new().into(),
        binary: false,
        truncated: false,
    })
}

/// Builds a bounded, Git-aware file index for quick-open.
///
/// This runs only when the user invokes quick-open. Git performs the traversal so ignored build
/// outputs and dependency folders never enter the index.
pub fn index_repository_files(root: &Path) -> Result<Vec<IndexedRepositoryFile>> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("Unable to resolve repository {}", root.display()))?;
    if !root.is_dir() {
        bail!("The selected repository is not a directory");
    }

    let output = Command::new("git")
        .current_dir(&root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
        ])
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("Unable to index files in {}", root.display()))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("Git could not index {}: {message}", root.display());
    }

    let mut files = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .take(MAXIMUM_INDEXED_REPOSITORY_FILES)
        .filter_map(|relative_bytes| {
            let relative = PathBuf::from(OsString::from_vec(relative_bytes.to_vec()));
            let path = root.join(&relative);
            let metadata = fs::symlink_metadata(&path).ok()?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return None;
            }
            Some(IndexedRepositoryFile {
                path,
                relative_path: relative.to_string_lossy().into_owned(),
            })
        })
        .collect::<Vec<_>>();
    files.sort_by_cached_key(|file| file.relative_path.to_ascii_lowercase());
    Ok(files)
}

/// Reads exactly one directory level. Descendants are intentionally deferred until the user
/// expands them so opening a repository never scales with the size of the whole checkout.
pub fn read_directory(path: &Path) -> Result<Vec<FileEntry>> {
    let directory =
        fs::read_dir(path).with_context(|| format!("Unable to read {}", path.display()))?;
    let mut entries = Vec::new();

    for entry in directory {
        let entry =
            entry.with_context(|| format!("Unable to read an item in {}", path.display()))?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy().into_owned();
        let file_type = entry
            .file_type()
            .with_context(|| format!("Unable to inspect {}", entry.path().display()))?;
        let kind = if file_type.is_dir() {
            FileEntryKind::Directory
        } else if file_type.is_symlink() {
            // Symlinked directories remain leaves. This prevents cycles and keeps traversal
            // bounded without resolving paths outside the selected repository.
            FileEntryKind::Symlink
        } else {
            FileEntryKind::File
        };

        entries.push(FileEntry {
            path: entry.path(),
            hidden: is_hidden(&file_name),
            name,
            kind,
        });
    }

    entries.sort_by_cached_key(|entry| {
        (
            !entry.kind.is_directory(),
            entry.name.to_ascii_lowercase(),
            entry.name.clone(),
        )
    });
    Ok(entries)
}

/// Reads a regular UTF-8 text file that resolves inside the selected repository.
///
/// Keeping this boundary in the service prevents the editor from following a tree symlink out of
/// the repository and bounds the amount of text sent to the UI in one document.
pub fn read_text_file(root: &Path, path: &Path) -> Result<String> {
    let (_, path, metadata) = resolve_regular_file(root, path)?;
    if metadata.len() > MAXIMUM_EDITABLE_FILE_BYTES as u64 {
        bail!("The file is larger than 8 MiB");
    }

    let bytes = fs::read(&path).with_context(|| format!("Unable to read {}", path.display()))?;
    if bytes.contains(&0) {
        bail!("The file appears to be binary");
    }
    let line_count = bytes.iter().filter(|byte| **byte == b'\n').count() + 1;
    if line_count > MAXIMUM_EDITABLE_FILE_LINES {
        bail!("The file has more than 50,000 lines");
    }

    String::from_utf8(bytes).with_context(|| format!("{} is not UTF-8 text", path.display()))
}

/// Atomically replaces an editable file while preserving its Unix permission bits.
pub fn write_text_file(root: &Path, path: &Path, content: &str) -> Result<()> {
    if content.len() > MAXIMUM_EDITABLE_FILE_BYTES {
        bail!("The file is larger than 8 MiB");
    }
    if content
        .as_bytes()
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
        > MAXIMUM_EDITABLE_FILE_LINES
    {
        bail!("The file has more than 50,000 lines");
    }

    let (_, path, metadata) = resolve_regular_file(root, path)?;
    let parent = path
        .parent()
        .context("The file does not have a parent directory")?;
    let file_name = path.file_name().and_then(OsStr::to_str).unwrap_or("file");
    let temporary = parent.join(format!(".{file_name}.blackholes-{}.tmp", Uuid::new_v4()));
    let mode = metadata.permissions().mode();

    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .with_context(|| format!("Unable to create {}", temporary.display()))?;
        file.write_all(content.as_bytes())?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.sync_all()?;

        // Re-check the destination immediately before replacement so a symlink cannot silently
        // become the save target while the temporary file is being written.
        let current = fs::symlink_metadata(&path)?;
        if !current.is_file() || current.file_type().is_symlink() {
            bail!("The save target is no longer a regular file");
        }
        fs::rename(&temporary, &path)
            .with_context(|| format!("Unable to save {}", path.display()))?;
        Ok::<_, anyhow::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn repository_change_kind(x: char, y: char) -> RepositoryChangeKind {
    if x == '?' && y == '?' {
        return RepositoryChangeKind::Untracked;
    }
    if matches!(x, 'U') || matches!(y, 'U') || matches!((x, y), ('A', 'A') | ('D', 'D')) {
        return RepositoryChangeKind::Conflicted;
    }
    if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
        return RepositoryChangeKind::Renamed;
    }
    if x == 'D' || y == 'D' {
        return RepositoryChangeKind::Deleted;
    }
    if x == 'A' {
        return RepositoryChangeKind::Added;
    }
    RepositoryChangeKind::Modified
}

fn added_file_diff(root: &Path, path: &Path) -> Result<RepositoryFileDiff> {
    let content = read_text_file(root, path);
    let content = match content {
        Ok(content) => content,
        Err(error) if error.to_string().contains("binary") => {
            return Ok(RepositoryFileDiff {
                original: None, modified: None,
                rows: Vec::new().into(),
                binary: true,
                truncated: false,
            });
        }
        Err(error) => return Err(error),
    };
    let mut rows = Vec::new();
    rows.push(RepositoryDiffRow::Hunk {
        old_start: 0,
        new_start: 1,
        header: "New file".into(),
    });
    for (index, line) in content.lines().enumerate() {
        if rows.len() >= MAXIMUM_DIFF_ROWS {
            return Ok(RepositoryFileDiff {
                original: None, modified: None,
                rows: rows.into(),
                binary: false,
                truncated: true,
            });
        }
        rows.push(RepositoryDiffRow::Line {
            old_number: None,
            new_number: Some(index + 1),
            old_text: String::new(),
            new_text: line.to_string(),
            kind: RepositoryDiffLineKind::Added,
        });
    }
    Ok(RepositoryFileDiff {
        original: None, modified: None,
        rows: rows.into(),
        binary: false,
        truncated: false,
    })
}

fn parse_repository_patch(patch: &str) -> RepositoryFileDiff {
    let binary = patch.contains("GIT binary patch") || patch.contains("Binary files ");
    let mut rows = Vec::new();
    let mut old_number = 0;
    let mut new_number = 0;
    let mut inside_hunk = false;
    let mut deleted = Vec::<(usize, String)>::new();
    let mut added = Vec::<(usize, String)>::new();
    let mut truncated = false;

    for line in patch.lines() {
        if let Some((old_start, new_start)) = parse_hunk_starts(line) {
            flush_changed_lines(&mut rows, &mut deleted, &mut added);
            if rows.len() >= MAXIMUM_DIFF_ROWS {
                truncated = true;
                break;
            }
            old_number = old_start;
            new_number = new_start;
            inside_hunk = true;
            rows.push(RepositoryDiffRow::Hunk {
                old_start,
                new_start,
                header: line.to_string(),
            });
            continue;
        }
        if !inside_hunk || line == "\\ No newline at end of file" {
            continue;
        }

        if let Some(text) = line.strip_prefix(' ') {
            flush_changed_lines(&mut rows, &mut deleted, &mut added);
            if rows.len() >= MAXIMUM_DIFF_ROWS {
                truncated = true;
                break;
            }
            rows.push(RepositoryDiffRow::Line {
                old_number: Some(old_number),
                new_number: Some(new_number),
                old_text: text.to_string(),
                new_text: text.to_string(),
                kind: RepositoryDiffLineKind::Context,
            });
            old_number += 1;
            new_number += 1;
        } else if let Some(text) = line.strip_prefix('-') {
            deleted.push((old_number, text.to_string()));
            old_number += 1;
        } else if let Some(text) = line.strip_prefix('+') {
            added.push((new_number, text.to_string()));
            new_number += 1;
        }
    }
    flush_changed_lines(&mut rows, &mut deleted, &mut added);
    truncated |= rows.len() > MAXIMUM_DIFF_ROWS;
    rows.truncate(MAXIMUM_DIFF_ROWS);

    RepositoryFileDiff {
        original: None, modified: None,
        rows: rows.into(),
        binary,
        truncated,
    }
}

fn parse_hunk_starts(line: &str) -> Option<(usize, usize)> {
    if !line.starts_with("@@ ") {
        return None;
    }
    let mut parts = line.split_whitespace();
    parts.next()?;
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let old_start = old.split(',').next()?.parse().ok()?;
    let new_start = new.split(',').next()?.parse().ok()?;
    Some((old_start, new_start))
}

fn flush_changed_lines(
    rows: &mut Vec<RepositoryDiffRow>,
    deleted: &mut Vec<(usize, String)>,
    added: &mut Vec<(usize, String)>,
) {
    let line_count = deleted.len().max(added.len());
    for index in 0..line_count {
        let old = deleted.get(index);
        let new = added.get(index);
        let kind = match (old, new) {
            (Some(_), Some(_)) => RepositoryDiffLineKind::Changed,
            (Some(_), None) => RepositoryDiffLineKind::Deleted,
            (None, Some(_)) => RepositoryDiffLineKind::Added,
            (None, None) => continue,
        };
        rows.push(RepositoryDiffRow::Line {
            old_number: old.map(|(number, _)| *number),
            new_number: new.map(|(number, _)| *number),
            old_text: old.map(|(_, text)| text.clone()).unwrap_or_default(),
            new_text: new.map(|(_, text)| text.clone()).unwrap_or_default(),
            kind,
        });
    }
    deleted.clear();
    added.clear();
}

fn resolve_regular_file(root: &Path, path: &Path) -> Result<(PathBuf, PathBuf, fs::Metadata)> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("Unable to resolve repository {}", root.display()))?;
    if !root.is_dir() {
        bail!("The selected repository is not a directory");
    }

    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Unable to inspect {}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("The selected item is not a regular file");
    }
    let path =
        fs::canonicalize(path).with_context(|| format!("Unable to resolve {}", path.display()))?;
    if !path.starts_with(&root) {
        bail!("The selected file is outside the repository");
    }

    Ok((root, path, metadata))
}

fn is_hidden(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with('.') && name != "." && name != "..")
}
