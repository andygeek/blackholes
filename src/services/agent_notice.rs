// SPDX-License-Identifier: MPL-2.0
//! Bounded, plain-text notification copy. Never scrape terminal output or an
//! unrelated transcript: only Stop's original assistant message is eligible.

fn compact(text: &str, limit: usize) -> String {
    let mut output = String::new();
    let mut count = 0;
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() { space = !output.is_empty(); continue; }
        if c.is_control() || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') { continue; }
        if count + usize::from(space) >= limit {
            output.push('…');
            break;
        }
        if space { output.push(' '); count += 1; space = false; }
        output.push(c); count += 1;
    }
    output
}

pub fn response_preview(event: &str, answer: Option<&str>) -> Option<String> {
    if event != "Stop" { return None; }
    let answer = answer?;
    // Read a bounded prefix before cleaning common Markdown presentation. The
    // IPC message never contains a whole response, prompt, reasoning or tool log.
    let prefix: String = answer.chars().take(1024).collect();
    let text = prefix.lines().filter_map(|line| {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") { return None; }
        let heading = line.trim_start_matches('#');
        let line = if heading.len() != line.len() && heading.starts_with(' ') { heading.trim_start() } else { line };
        let line = ["- ", "* ", "> "].into_iter().find_map(|p| line.strip_prefix(p)).unwrap_or(line);
        Some(line.replace(['`', '*'], ""))
    }).collect::<Vec<_>>().join(" ");
    let mut preview = compact(&text, 180);
    if answer.chars().nth(1024).is_some() && !preview.ends_with('…') && !preview.is_empty() { preview.push('…'); }
    (!preview.is_empty()).then_some(preview)
}

pub fn title(project: Option<&str>, context: &str) -> String {
    let same_context = project.is_some_and(|name| name.trim() == context.trim());
    let project = compact(project.unwrap_or_default(), 56);
    let context = compact(context, 72);
    match (project.is_empty(), context.is_empty() || same_context || project == context) {
        (false, true) => project,
        (false, false) => format!("{project} · {context}"),
        (true, false) => context,
        (true, true) => "Blackholes".into(),
    }
}
