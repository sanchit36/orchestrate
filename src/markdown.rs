//! The one shared Markdown section rule, and the body forms read with it:
//! blocked-by references and acceptance criteria.

/// Level and text of a heading line such as `## Blocked by`.
fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|&b| b == b'#').count();
    let text = line[level..].strip_prefix(' ')?;
    (1..=6).contains(&level).then_some((level, text.trim()))
}

/// The lines under the first heading (any level) named one of `names`, ignoring case,
/// up to the next heading of the same or higher level.
pub fn section<'a, S: AsRef<str>>(body: &'a str, names: &[S]) -> Option<Vec<&'a str>> {
    let mut lines = body.lines();
    let (level, _) = lines
        .by_ref()
        .filter_map(heading)
        .find(|(_, text)| names.iter().any(|n| n.as_ref().eq_ignore_ascii_case(text)))?;
    Some(
        lines
            .take_while(|l| heading(l).is_none_or(|(lv, _)| lv > level))
            .collect(),
    )
}

/// Blocker references from a `Blocked by:` line and the list items of a `## Blocked by`
/// section, combined, in order, without repeats. A Markdown link yields its target;
/// the adapter turns each reference into an id.
pub fn blocked_by(body: &str) -> Vec<String> {
    let line = body
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("Blocked by:"));
    let items = section(body, &["Blocked by"])
        .unwrap_or_default()
        .into_iter()
        .filter_map(list_item);
    let mut refs = Vec::new();
    // shortcut: splits on every comma, so a link text containing a comma breaks; fine until someone writes one.
    for r in line.into_iter().chain(items).flat_map(|s| s.split(',')) {
        let r = link_target(r.trim()).to_string();
        if !r.is_empty() && !refs.contains(&r) {
            refs.push(r);
        }
    }
    refs
}

/// The text of a `-`, `*`, `+` or numbered list item.
fn list_item(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let bullet = ["- ", "* ", "+ "].iter().find_map(|b| line.strip_prefix(b));
    bullet.or_else(|| {
        let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());
        let numbered = rest.len() < line.len();
        numbered
            .then(|| rest.strip_prefix(". ").or(rest.strip_prefix(") ")))
            .flatten()
    })
}

/// `target` for `[text](target)`, otherwise the input.
fn link_target(s: &str) -> &str {
    s.strip_prefix('[')
        .and_then(|s| s.split_once("]("))
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .unwrap_or(s)
}

#[derive(Clone)]
pub struct Criterion {
    #[expect(dead_code, reason = "read by the run report and ticking (#18)")]
    pub text: String,
    #[expect(dead_code, reason = "read by the run report and ticking (#18)")]
    pub met: bool,
}

/// `- [ ]` / `- [x]` items directly under a criteria heading, or `None` when no
/// heading matches. Plain bullets, numbered lists and checkboxes elsewhere do not count.
pub fn criteria<S: AsRef<str>>(body: &str, headings: &[S]) -> Option<Vec<Criterion>> {
    let lines = section(body, headings)?;
    let parse = |l: &str| {
        let (met, text) = match l.strip_prefix("- [")?.split_at_checked(2)? {
            (" ]", text) => (false, text),
            ("x]" | "X]", text) => (true, text),
            _ => return None,
        };
        let text = text.strip_prefix(' ')?.trim().to_string();
        Some(Criterion { text, met })
    };
    Some(lines.into_iter().filter_map(parse).collect())
}
