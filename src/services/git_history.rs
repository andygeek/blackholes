//! Local history and explicit, non-forcing remote operations for Source Control.
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::{io::Read, path::{Path, PathBuf}, process::{Command, Stdio}, sync::Arc, time::{Duration, Instant}};
use super::files::{RepositoryChange, RepositoryChangeKind, RepositoryFileDiff, parse_repository_patch};

#[derive(Clone, Serialize)]
pub struct GitRef { pub name: String, pub kind: String }
#[derive(Clone, Serialize)]
pub struct Commit {
    pub id: String, pub parents: Vec<String>, pub author: String, pub timestamp: i64,
    pub subject: String, pub refs: Vec<GitRef>, pub outgoing: bool,
}
#[derive(Clone, Serialize)]
pub struct Snapshot {
    pub commits: Vec<Commit>, pub has_more: bool, pub head: Option<String>, pub branch: Option<String>,
    pub upstream: Option<String>, pub ahead: usize, pub behind: usize, pub remotes: Vec<String>,
    pub push_remote: Option<String>, pub push_branch: Option<String>,
    #[serde(skip)] pub git_dirs: Vec<PathBuf>,
}
#[derive(Clone, Serialize)]
pub struct CommitFile { pub relative_path: String, pub previous_relative_path: Option<String>, pub kind: String }
impl CommitFile {
    pub fn change(&self, root: &Path) -> RepositoryChange {
        RepositoryChange { path: root.join(&self.relative_path), relative_path: self.relative_path.clone(),
            previous_relative_path: self.previous_relative_path.clone(), index_status: ' ', worktree_status: ' ', kind: match self.kind.as_str() {
                "added" => RepositoryChangeKind::Added, "deleted" => RepositoryChangeKind::Deleted,
                "renamed" => RepositoryChangeKind::Renamed, _ => RepositoryChangeKind::Modified,
            } }
    }
}
#[derive(Clone, Serialize)]
pub struct CommitDetail {
    pub id: String, pub parents: Vec<String>, pub parent: Option<String>, pub author: String,
    pub timestamp: i64, pub message: String, pub files: Vec<CommitFile>,
}

// Drain both pipes concurrently, bound memory and terminate helpers on timeout.
// Errors from remote commands are classified below rather than exposing URLs/tokens.
pub(super) fn run_git(root: &Path, args: &[&str], network: bool, limit: usize, envs: &[(&str, &std::ffi::OsStr)]) -> Result<std::process::Output> {
    use std::os::unix::process::CommandExt;
    let mut child = Command::new("git")
        .args(["--no-pager", "--literal-pathspecs", "-c", "color.ui=false", "-c", "diff.relative=false"])
        .args(args).envs(envs.iter().copied()).current_dir(root).env("GIT_TERMINAL_PROMPT", "0").env("GCM_INTERACTIVE", "never")
        .env("GIT_OPTIONAL_LOCKS", "0").env("LC_ALL", "C")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0)
        .spawn().context("Could not start Git")?;
    let stdout = child.stdout.take().context("Git stdout unavailable")?;
    let stderr = child.stderr.take().context("Git stderr unavailable")?;
    let (tx, rx) = std::sync::mpsc::channel();
    let out_tx = tx.clone();
    std::thread::spawn(move || { let mut data = Vec::new(); let result = stdout.take(limit as u64 + 1).read_to_end(&mut data); let _ = out_tx.send((true, result.map(|_| data))); });
    std::thread::spawn(move || { let mut data = Vec::new(); let result = stderr.take(65537).read_to_end(&mut data); let _ = tx.send((false, result.map(|_| data))); });
    let deadline = Instant::now() + Duration::from_secs(if network { 120 } else { 20 });
    let mut output = None;
    let mut error = None;
    let status = loop {
        while let Ok((is_out, result)) = rx.try_recv() {
            let bytes = result.unwrap_or_default();
            if is_out { output = Some(bytes); } else { error = Some(bytes); }
        }
        if output.as_ref().is_some_and(|b| b.len() > limit) || error.as_ref().is_some_and(|b| b.len() > 65536) || Instant::now() >= deadline {
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL); }
            let _ = child.wait();
            bail!("Git exceeded its time or output limit. Use the terminal for this operation.");
        }
        match child.try_wait()? {
            Some(status) if output.is_some() && error.is_some() => break status,
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let _ = child.wait();
    Ok(std::process::Output { status, stdout: output.unwrap_or_default(), stderr: error.unwrap_or_default() })
}

pub(super) fn git(root: &Path, args: &[&str], network: bool, limit: usize) -> Result<Vec<u8>> {
    let output = run_git(root, args, network, limit, &[])?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        if network {
            if message.contains("non-fast-forward") || message.contains("fetch first") || message.contains("[rejected]") {
                bail!("Push rejected: the remote has changes. Fetch and ask your agent to reconcile the branches before retrying.");
            }
            bail!("Git remote operation failed. Check access and authentication in your terminal, then refresh and retry.");
        }
        if message.contains("not a git repository") { bail!("This folder is not a Git repository."); }
        bail!("Git could not read this revision. Refresh the graph and retry.");
    }
    Ok(output.stdout)
}
fn read(root: &Path, args: &[&str]) -> Result<String> { Ok(String::from_utf8(git(root, args, false, 8 * 1024 * 1024)?)?) }
fn optional(root: &Path, args: &[&str]) -> Option<String> { read(root, args).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()) }
fn oid(id: &str) -> Result<()> { ensure!([40, 64].contains(&id.len()) && id.bytes().all(|c| c.is_ascii_hexdigit()), "Invalid commit identifier"); Ok(()) }

pub fn snapshot(root: &Path, limit: usize) -> Result<Snapshot> {
    read(root, &["rev-parse", "--show-toplevel"])?;
    let head = optional(root, &["rev-parse", "--verify", "HEAD"]);
    let branch = optional(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]);
    let remotes = read(root, &["remote"])?.lines().map(str::to_owned).collect::<Vec<_>>();
    let upstream = optional(root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]);
    let mut ahead = 0;
    let mut behind = 0;
    let mut outgoing = std::collections::HashSet::new();
    if upstream.is_some() && head.is_some() {
        let counts = read(root, &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"])?;
        let mut counts = counts.split_whitespace();
        ahead = counts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        behind = counts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        outgoing.extend(read(root, &["rev-list", "--max-count=1000", "@{upstream}..HEAD"])?.lines().map(str::to_owned));
    }
    let push_remote = branch.as_ref().and_then(|b| optional(root, &["config", "--get", &format!("branch.{b}.remote")])).filter(|r| remotes.contains(r));
    let push_branch = branch.as_ref().and_then(|b| optional(root, &["config", "--get", &format!("branch.{b}.merge")])).and_then(|b| b.strip_prefix("refs/heads/").map(str::to_owned));
    let mut refs = std::collections::HashMap::<String, Vec<GitRef>>::new();
    for line in read(root, &["for-each-ref", "--format=%(objectname)%00%(refname)%00%(*objectname)", "refs/heads", "refs/remotes", "refs/tags"])?.lines() {
        let parts = line.split('\0').collect::<Vec<_>>();
        if parts.len() != 3 || parts[1].ends_with("/HEAD") { continue; }
        let (kind, name) = if let Some(n) = parts[1].strip_prefix("refs/heads/") { ("branch", n) }
            else if let Some(n) = parts[1].strip_prefix("refs/remotes/") { ("remote", n) }
            else { ("tag", parts[1].trim_start_matches("refs/tags/")) };
        refs.entry(if parts[2].is_empty() { parts[0] } else { parts[2] }.to_owned()).or_default().push(GitRef { name: name.into(), kind: kind.into() });
    }
    let max_count = format!("--max-count={}", limit.clamp(1, 1000) + 1);
    let mut args = vec!["log", "--topo-order", "--no-decorate", "--no-show-signature", "--encoding=UTF-8", "--branches", "--remotes", "--tags", &max_count, "--format=%H%x00%P%x00%an%x00%at%x00%s%x00"];
    if head.is_some() { args.push("HEAD"); }
    args.push("--");
    let raw = if head.is_some() || !refs.is_empty() { read(root, &args)? } else { String::new() };
    let mut commits = raw.split('\0').collect::<Vec<_>>().chunks_exact(5).map(|p| {
        let id = p[0].trim().to_owned();
        Commit { parents: p[1].split_whitespace().map(str::to_owned).collect(), author: p[2].into(), timestamp: p[3].parse().unwrap_or(0),
            subject: p[4].into(), refs: refs.remove(&id).unwrap_or_default(), outgoing: outgoing.contains(&id), id }
    }).collect::<Vec<_>>();
    let has_more = commits.len() > limit;
    commits.truncate(limit);
    let mut git_dirs = Vec::new();
    for flag in ["--git-dir", "--git-common-dir"] {
        let dir = PathBuf::from(read(root, &["rev-parse", "--path-format=absolute", flag])?.trim());
        if !git_dirs.contains(&dir) { git_dirs.push(dir); }
    }
    Ok(Snapshot { commits, has_more, head, branch, upstream, ahead, behind, remotes, push_remote, push_branch, git_dirs })
}

pub fn detail(root: &Path, id: &str, parent: Option<&str>) -> Result<CommitDetail> {
    oid(id)?;
    let raw = read(root, &["show", "-s", "--no-show-signature", "--encoding=UTF-8", "--format=%P%x00%an%x00%at%x00%B", id, "--"])?;
    let parts = raw.splitn(4, '\0').collect::<Vec<_>>();
    ensure!(parts.len() == 4, "Incomplete commit data");
    let parents = parts[0].split_whitespace().map(str::to_owned).collect::<Vec<_>>();
    let parent = parent.map(str::to_owned).or_else(|| parents.first().cloned());
    ensure!(parent.as_ref().is_none_or(|p| parents.contains(p)), "Not a parent of this commit");
    let mut args = vec!["diff-tree", "--no-commit-id", "--root", "-r", "--no-ext-diff", "-M", "--name-status", "-z"];
    if let Some(parent) = &parent { args.push(parent); }
    args.extend([id, "--"]);
    let raw = git(root, &args, false, 8 * 1024 * 1024)?;
    // Preserve NUL-delimited paths, including spaces, tabs and newlines.
    let mut fields = raw.split(|b| *b == 0).filter(|b| !b.is_empty());
    let mut files = Vec::new();
    while let Some(status) = fields.next() {
        let path = fields.next().context("Missing changed path")?;
        let previous = if matches!(status.first(), Some(b'R') | Some(b'C')) { Some(String::from_utf8(path.to_vec())?) } else { None };
        let path = if previous.is_some() { fields.next().context("Missing renamed path")? } else { path };
        files.push(CommitFile { relative_path: String::from_utf8(path.to_vec()).context("This commit contains a non-UTF-8 file name")?, previous_relative_path: previous,
            kind: match status.first() { Some(b'A') => "added", Some(b'D') => "deleted", Some(b'R') => "renamed", _ => "modified" }.into() });
    }
    files.sort_by(|a,b| a.relative_path.cmp(&b.relative_path));
    Ok(CommitDetail { id: id.into(), parent, parents, author: parts[1].into(), timestamp: parts[2].parse().unwrap_or(0), message: parts[3].trim_end().into(), files })
}

pub fn file_diff(root: &Path, detail: &CommitDetail, file: &CommitFile) -> Result<RepositoryFileDiff> {
    let mut args = vec!["diff-tree", "--no-commit-id", "--root", "-r", "-p", "--no-ext-diff", "--no-textconv", "--no-color", "-M", "--unified=3"];
    if let Some(parent) = &detail.parent { args.push(parent); }
    args.extend([&detail.id, "--", &file.relative_path]);
    if let Some(previous) = &file.previous_relative_path { args.push(previous); }
    let patch = git(root, &args, false, 16 * 1024 * 1024)?;
    let mut diff = parse_repository_patch(&String::from_utf8_lossy(&patch));
    fn blob(root: &Path, revision: &str, path: &str) -> Result<Arc<str>> {
        let bytes = git(root, &["cat-file", "blob", &format!("{revision}:{path}")], false, 8 * 1024 * 1024)?;
        ensure!(!bytes.contains(&0), "Binary file");
        let text = String::from_utf8(bytes)?;
        ensure!(text.lines().take(50_001).count() <= 50_000, "File too large");
        Ok(Arc::from(text))
    }
    if !diff.binary {
        let original = if file.kind == "added" || detail.parent.is_none() { Ok(Arc::from("")) }
            else { blob(root, detail.parent.as_deref().unwrap(), file.previous_relative_path.as_deref().unwrap_or(&file.relative_path)) };
        let modified = if file.kind == "deleted" { Ok(Arc::from("")) } else { blob(root, &detail.id, &file.relative_path) };
        if let (Ok(original), Ok(modified)) = (original, modified) { diff.original = Some(original); diff.modified = Some(modified); }
    }
    Ok(diff)
}

pub fn fetch(root: &Path, remote: &str) -> Result<()> {
    ensure!(read(root, &["remote"])?.lines().any(|r| r == remote) && !remote.starts_with('-'), "Unknown remote");
    git(root, &["fetch", "--no-recurse-submodules", "--", remote], true, 1024 * 1024)?;
    Ok(())
}

pub fn push(root: &Path, expected: &Snapshot, remote: &str) -> Result<()> {
    let head = expected.head.as_deref().context("No commits to push")?;
    let branch = expected.branch.as_deref().context("Checkout a branch before pushing")?;
    let current = snapshot(root, 1)?;
    ensure!(current.head == expected.head && current.branch == expected.branch && current.upstream == expected.upstream
        && current.push_remote == expected.push_remote && current.push_branch == expected.push_branch,
        "The branch changed since the push preview. Refresh and review it again.");
    ensure!(current.remotes.iter().any(|r| r == remote) && !remote.starts_with('-'), "Unknown remote");
    ensure!(current.upstream.is_none() || current.push_remote.as_deref() == Some(remote), "Push must use the branch's upstream remote");
    let target = if current.upstream.is_some() { current.push_branch.as_deref().context("Unsupported upstream branch")? } else { branch };
    read(root, &["check-ref-format", &format!("refs/heads/{target}")])?;
    // An immutable source OID protects against concurrent agent commits. Explicit
    // refspec, no force and no tags prevent push.default/followTags surprises.
    git(root, &["-c", &format!("remote.{remote}.mirror=false"), "push", "--no-force", "--no-follow-tags", "--recurse-submodules=no", "--", remote, &format!("{head}:refs/heads/{target}")], true, 1024 * 1024)?;
    // Establish tracking only if the same local branch is still checked out.
    if expected.upstream.is_none() && optional(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).as_deref() == Some(branch) {
        read(root, &["config", &format!("branch.{branch}.remote"), remote])
            .context("The branch was pushed, but its upstream could not be configured. Set tracking in your terminal.")?;
        read(root, &["config", &format!("branch.{branch}.merge"), &format!("refs/heads/{target}")])
            .context("The branch was pushed, but its upstream could not be configured. Set tracking in your terminal.")?;
    }
    Ok(())
}
