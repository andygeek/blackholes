use anyhow::{Result, ensure};
use regex::{Regex, RegexBuilder, RegexSet};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::atomic::{AtomicBool, Ordering}, time::{Duration, Instant}};
use super::files::{index_repository_files, read_text_file};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Options {
    pub query: String, pub case_sensitive: bool, pub whole_word: bool, pub regex: bool,
    pub include: String, pub exclude: String,
}
#[derive(Clone, Serialize)]
pub struct Match {
    pub line: usize, pub column: usize, pub end_column: usize,
    pub before: String, pub matched: String, pub after: String,
}
#[derive(Clone, Serialize)]
pub struct FileMatches { pub path: String, pub matches: Vec<Match> }
#[derive(Clone, Default, Serialize)]
pub struct SearchResult { pub files: Vec<FileMatches>, pub count: usize, pub limited: bool, pub skipped: usize }

fn patterns(input: &str) -> Result<Option<RegexSet>> {
    if input.trim().is_empty() { return Ok(None); }
    ensure!(input.len() <= 4096, "File filters are too long");
    let mut patterns = Vec::new();
    for glob in input.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let glob = glob.trim_start_matches("./");
        let mut expression = if glob.contains('/') { "^" } else { "(?:^|/)" }.to_string();
        let mut chars = glob.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '*' if chars.peek() == Some(&'*') => {
                    chars.next();
                    if chars.peek() == Some(&'/') { chars.next(); expression.push_str("(?:.*/)?"); } else { expression.push_str(".*"); }
                }
                '*' => expression.push_str("[^/]*"),
                '?' => expression.push_str("[^/]"),
                _ => expression.push_str(&regex::escape(&ch.to_string())),
            }
        }
        expression.push_str(if glob.ends_with('/') { ".*$" } else { "(?:/.*)?$" });
        patterns.push(expression);
    }
    Ok(Some(RegexSet::new(patterns)?))
}
fn expression(options: &Options) -> Result<Regex> {
    ensure!(options.query.len() <= 4096, "Search is limited to 4,096 bytes");
    let query = if options.regex { options.query.clone() } else { regex::escape(&options.query) };
    let query = if options.whole_word { format!(r"\b(?:{query})\b") } else { query };
    Ok(RegexBuilder::new(&query).case_insensitive(!options.case_sensitive).size_limit(1024 * 1024).build()?)
}

pub fn search(root: &Path, options: &Options, cancelled: &AtomicBool) -> Result<SearchResult> {
    if options.query.is_empty() { return Ok(SearchResult::default()); }
    let matcher = expression(options)?;
    let include = patterns(&options.include)?;
    let exclude = patterns(&options.exclude)?;
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut files = index_repository_files(root)?;
    files.dedup_by(|a, b| a.path == b.path);
    let mut result = SearchResult { limited: files.len() >= 50_000, ..Default::default() };
    let mut bytes_read = 0usize;
    for file in files {
        if cancelled.load(Ordering::Relaxed) { return Ok(SearchResult::default()); }
        if Instant::now() > deadline || bytes_read >= 256 * 1024 * 1024 { result.limited = true; break; }
        if file.path.to_str().is_none() { result.skipped += 1; continue; }
        if include.as_ref().is_some_and(|p| !p.is_match(&file.relative_path)) || exclude.as_ref().is_some_and(|p| p.is_match(&file.relative_path)) { continue; }
        let content = match read_text_file(root, &file.path) { Ok(content) => content, Err(_) => { result.skipped += 1; continue; } };
        bytes_read += content.len();
        let mut matches = Vec::new();
        for (index, line) in content.lines().enumerate() {
            if cancelled.load(Ordering::Relaxed) { return Ok(SearchResult::default()); }
            if Instant::now() > deadline { result.limited = true; break; }
            for found in matcher.find_iter(line) {
                let before = line[..found.start()].chars().rev().take(60).collect::<Vec<_>>().into_iter().rev().collect();
                let matched = found.as_str().chars().take(240).collect();
                let after = line[found.end()..].chars().take(100).collect();
                matches.push(Match { line: index + 1, column: line[..found.start()].encode_utf16().count() + 1,
                    end_column: line[..found.end()].encode_utf16().count() + 1, before, matched, after });
                result.count += 1;
                if result.count >= 2000 { result.limited = true; break; }
            }
            if result.count >= 2000 { break; }
        }
        if !matches.is_empty() { result.files.push(FileMatches { path: file.relative_path, matches }); }
        if result.count >= 2000 || Instant::now() > deadline { break; }
    }
    Ok(result)
}
