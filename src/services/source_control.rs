use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::{collections::hash_map::DefaultHasher, fs, hash::{Hash, Hasher}, io::Write,
    os::unix::fs::OpenOptionsExt, path::{Path, PathBuf}, sync::Arc};
use super::{files::{self, RepositoryChange, RepositoryChangeKind as Kind, RepositoryFileDiff}, git_history::{git, run_git}};

#[derive(Clone, Default, PartialEq, Eq, Serialize)]
pub struct IndexInfo {
    pub token: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub blocked: Option<String>,
}
pub struct Status { pub changes: Vec<RepositoryChange>, pub info: IndexInfo }

fn text(root: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8(git(root, args, false, 16 * 1024 * 1024)?)?.trim_end_matches('\n').into())
}
fn optional(root: &Path, args: &[&str]) -> Option<String> {
    text(root, args).ok().filter(|s| !s.is_empty())
}
fn git_path(root: &Path, name: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(text(root, &["rev-parse", "--path-format=absolute", "--git-path", name])?))
}
fn info(root: &Path) -> Result<IndexInfo> {
    let top = text(root, &["rev-parse", "--show-toplevel"])?;
    let head = optional(root, &["rev-parse", "--verify", "HEAD"]);
    let branch = optional(root, &["symbolic-ref", "--quiet", "HEAD"]);
    let index = git(root, &["ls-files", "--stage", "-z"], false, 16 * 1024 * 1024)?;
    let mut hasher = DefaultHasher::new();
    index.hash(&mut hasher); head.hash(&mut hasher); branch.hash(&mut hasher);
    let mut blocked = if fs::canonicalize(root)? != fs::canonicalize(top)? {
        Some("Open the repository root to stage files and commit.".into())
    } else { None };
    let git_dir = PathBuf::from(text(root, &["rev-parse", "--absolute-git-dir"])?);
    for name in ["MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "rebase-merge", "rebase-apply", "sequencer"] {
        if git_dir.join(name).exists() { blocked = Some("Finish the ongoing merge, rebase or cherry-pick in your terminal first.".into()); break; }
    }
    Ok(IndexInfo { token: format!("{:016x}", hasher.finish()), head, branch, blocked })
}
pub fn status(root: &Path) -> Result<Status> {
    let before = info(root)?;
    let changes = files::repository_changes(root)?;
    let after = info(root)?;
    ensure!(before == after, "Git changed while reading the index. Refresh Changes and retry.");
    Ok(Status { changes, info: after })
}
pub fn side_kind(change: &RepositoryChange, staged: bool) -> Option<Kind> {
    if change.kind == Kind::Conflicted { return (!staged).then_some(Kind::Conflicted); }
    match if staged { change.index_status } else { change.worktree_status } {
        'A' | 'C' => Some(Kind::Added), 'D' => Some(Kind::Deleted), 'R' => Some(Kind::Renamed),
        '?' if !staged => Some(Kind::Untracked), 'M' | 'T' | 'm' | '?' => (!staged || change.index_status != '?').then_some(Kind::Modified),
        _ => None,
    }
}
pub fn side_change(change: &RepositoryChange, staged: bool) -> Option<RepositoryChange> {
    let mut result = change.clone();
    result.kind = side_kind(change, staged)?;
    if result.kind != Kind::Renamed { result.previous_relative_path = None; }
    Some(result)
}

struct Temporary(PathBuf);
impl Drop for Temporary { fn drop(&mut self) { let _ = fs::remove_file(&self.0); } }
fn temporary(directory: &Path, content: &[u8]) -> Result<Temporary> {
    let path = directory.join(format!("blackholes-{}", uuid::Uuid::new_v4()));
    let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
    let guard = Temporary(path);
    file.write_all(content)?;
    Ok(guard)
}
fn mutation(root: &Path, args: &[&str], envs: &[(&str, &std::ffi::OsStr)]) -> Result<()> {
    let result = run_git(root, args, true, 1024 * 1024, envs)?;
    if !result.status.success() {
        let output = if result.stderr.is_empty() { &result.stdout } else { &result.stderr };
        let message: String = String::from_utf8_lossy(output).chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').take(6000).collect();
        bail!("Git did not complete the operation:\n{}", message.trim());
    }
    Ok(())
}

pub fn stage(root: &Path, token: &str, selected: Option<&str>, staged: bool) -> Result<()> {
    let status = status(root)?;
    ensure!(status.info.token == token, "The index changed. Review the refreshed file list and retry.");
    ensure!(status.info.blocked.is_none(), "{}", status.info.blocked.unwrap_or_default());
    let mut paths = Vec::new();
    for change in &status.changes {
        if selected.is_some_and(|p| p != change.relative_path) { continue; }
        let Some(change) = side_change(change, !staged) else { continue; };
        paths.push(change.relative_path.clone());
        if let Some(old) = &change.previous_relative_path { paths.push(old.clone()); }
    }
    ensure!(!paths.is_empty(), "The selected files no longer have changes. Refresh and retry.");
    paths.sort(); paths.dedup();
    let data = paths.iter().flat_map(|path| path.as_bytes().iter().copied().chain(std::iter::once(0))).collect::<Vec<_>>();
    let index_path = git_path(root, "index")?;
    let pathspec = temporary(index_path.parent().context("Missing Git directory")?, &data)?;
    let argument = format!("--pathspec-from-file={}", pathspec.0.display());
    if staged {
        mutation(root, &["add", "--all", &argument, "--pathspec-file-nul"], &[])
    } else if status.info.head.is_some() {
        mutation(root, &["restore", "--staged", "--source=HEAD", &argument, "--pathspec-file-nul"], &[])
    } else {
        // An unborn branch has no HEAD to restore. Removing index entries does
        // not remove any working files, including files edited after staging.
        mutation(root, &["rm", "--cached", "--force", "--ignore-unmatch", &argument, "--pathspec-file-nul"], &[])
    }
}

pub fn commit(root: &Path, token: &str, message: &str) -> Result<String> {
    ensure!(!message.trim().is_empty() && message.len() <= 65536 && !message.contains('\0'), "Enter a commit message (up to 64 KiB).");
    let status = status(root)?;
    ensure!(status.info.token == token, "The staged changes or branch changed. Review them before committing.");
    ensure!(status.info.blocked.is_none(), "{}", status.info.blocked.unwrap_or_default());
    ensure!(status.changes.iter().all(|c| c.kind != Kind::Conflicted), "Resolve the conflicted files before committing.");
    ensure!(status.changes.iter().any(|c| side_kind(c, true).is_some()), "Stage at least one file before committing.");
    let index = git_path(root, "index")?;
    let mut lock_name = index.as_os_str().to_owned(); lock_name.push(".lock");
    let lock_path = PathBuf::from(lock_name);
    let mut lock = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&lock_path)
        .context("Git is busy (index.lock exists). Wait for the other Git operation to finish.")?;
    let guard = Temporary(lock_path.clone());
    lock.write_all(&fs::read(&index)?)?;
    fs::set_permissions(&lock_path, fs::metadata(&index)?.permissions())?;
    drop(lock);
    // Hold the real index lock while Git commits its private snapshot. Other
    // agents cannot stage extra files between validation and Git's own commit.
    ensure!(info(root)?.token == token, "The index changed before it could be locked. Review Changes and retry.");
    let message_file = temporary(index.parent().context("Missing Git directory")?, message.as_bytes())?;
    let file_arg = format!("--file={}", message_file.0.display());
    let mut nested_lock = lock_path.as_os_str().to_owned(); nested_lock.push(".lock");
    let nested_lock = PathBuf::from(nested_lock);
    ensure!(!nested_lock.exists(), "A Git snapshot lock already exists. Finish the other Git operation before retrying.");
    let _nested_guard = Temporary(nested_lock);
    let result = mutation(root, &["commit", &file_arg, "--cleanup=whitespace"], &[("GIT_INDEX_FILE", lock_path.as_os_str())]);
    // Keep the normal Git behavior if a hook updates the index, even if it
    // rejects the commit. No working-tree files are restored or discarded.
    fs::rename(&lock_path, &index).context("Could not publish Git's updated index")?;
    drop(guard);
    result?;
    text(root, &["rev-parse", "HEAD"])
}

pub fn diff(root: &Path, change: &RepositoryChange, staged: bool) -> Result<RepositoryFileDiff> {
    let change = side_change(change, staged).context("This file no longer has changes in this group")?;
    if !staged && change.kind == Kind::Untracked { return files::repository_file_diff(root, &change); }
    if change.kind == Kind::Conflicted { bail!("Resolve the conflict in the file or terminal, then stage the result."); }
    let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-color", "--find-renames", "--unified=3"];
    if staged { args.push("--cached"); }
    args.extend(["--", &change.relative_path]);
    if let Some(previous) = &change.previous_relative_path { args.push(previous); }
    let bytes = git(root, &args, false, 16 * 1024 * 1024)?;
    let mut diff = files::parse_repository_patch(&String::from_utf8_lossy(&bytes));
    fn blob(root: &Path, revision: &str, path: &str) -> Result<Arc<str>> {
        let bytes = git(root, &["cat-file", "blob", &format!("{revision}:{path}")], false, 8 * 1024 * 1024)?;
        ensure!(!bytes.contains(&0), "Binary file");
        let content = String::from_utf8(bytes)?;
        ensure!(content.lines().take(50_001).count() <= 50_000, "Large file");
        Ok(Arc::from(content))
    }
    if !diff.binary {
        let original = if change.kind == Kind::Added { Ok(Arc::from("")) }
            else { blob(root, if staged { "HEAD" } else { "" }, change.previous_relative_path.as_deref().unwrap_or(&change.relative_path)) };
        let modified = if change.kind == Kind::Deleted { Ok(Arc::from("")) }
            else if staged { blob(root, "", &change.relative_path) }
            else { files::read_text_file(root, &change.path).map(Arc::from) };
        if let (Ok(original), Ok(modified)) = (original, modified) { diff.original = Some(original); diff.modified = Some(modified); }
    }
    Ok(diff)
}
