//! Live Markdown for the native notes editor: which parts of the text are
//! markers, headings, emphasis, code, #tags, checklist boxes. A plain scan, line by
//! line, in UTF-16 offsets (what NSString ranges count in). The text itself is
//! never changed: the notes stay the Markdown they are.

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// The text of a heading of this level (the `# ` before it is a Marker).
    Heading(u8),
    /// Syntax that is only there to be typed: `# `, `**`, a backtick, `> `.
    Marker,
    Bold,
    Italic,
    Strike,
    Code,
    CodeBlock,
    Quote,
    /// A tag, whole, with its `#`.
    Tag(String),
    /// The `- [ ] ` or `- [x] ` that starts a checklist line.
    Box { checked: bool },
    /// The text of a ticked checklist line.
    Done,
    /// `![alt](url)`, whole.
    Image,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// All the spans of `text`. Spans of a line never cross it.
pub fn parse(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut base = 0usize; // UTF-16 offset of the line's start
    let mut in_fence = false;
    for line in text.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        // Offsets within the line, in UTF-16 units: off[i] is where chars[i] starts.
        let mut off = Vec::with_capacity(chars.len() + 1);
        let mut u = 0usize;
        for c in &chars {
            off.push(u);
            u += c.len_utf16();
        }
        off.push(u);
        let line_units = u;
        let span = |from: usize, to: usize, kind: Kind| Span {
            start: base + off[from],
            end: base + off[to],
            kind,
        };
        let trimmed_fence = line.trim_start().starts_with("```");
        if trimmed_fence {
            in_fence = !in_fence;
            out.push(span(0, chars.len(), Kind::Marker));
            base += line_units + 1;
            continue;
        }
        if in_fence {
            if !chars.is_empty() {
                out.push(span(0, chars.len(), Kind::CodeBlock));
            }
            base += line_units + 1;
            continue;
        }
        // Block marker.
        let indent = chars.iter().take_while(|c| **c == ' ' || **c == '\t').count();
        let rest: String = chars[indent..].iter().collect();
        let mut body = indent; // where inline text starts
        let hashes = chars[indent..].iter().take_while(|c| **c == '#').count();
        if (1..=6).contains(&hashes)
            && chars.get(indent + hashes) == Some(&' ')
        {
            out.push(span(indent, indent + hashes + 1, Kind::Marker));
            body = indent + hashes + 1;
            if body < chars.len() {
                out.push(span(body, chars.len(), Kind::Heading(hashes.min(3) as u8)));
            }
        } else if rest.starts_with("- [ ] ") || rest.starts_with("- [x] ") || rest.starts_with("- [X] ")
            || rest.starts_with("* [ ] ") || rest.starts_with("* [x] ")
        {
            let checked = !rest.starts_with("- [ ] ") && !rest.starts_with("* [ ] ");
            out.push(span(indent, indent + 6, Kind::Box { checked }));
            body = indent + 6;
            if checked && body < chars.len() {
                out.push(span(body, chars.len(), Kind::Done));
            }
        } else if rest.starts_with("- ") || rest.starts_with("* ") || rest.starts_with("+ ") {
            out.push(span(indent, indent + 2, Kind::Marker));
            body = indent + 2;
        } else if let Some(n) = ordered_marker(&chars[indent..]) {
            out.push(span(indent, indent + n, Kind::Marker));
            body = indent + n;
        } else if rest.starts_with("> ") {
            out.push(span(indent, indent + 2, Kind::Marker));
            body = indent + 2;
            if body < chars.len() {
                out.push(span(body, chars.len(), Kind::Quote));
            }
        }
        inline(&chars, body, &span, &mut out);
        base += line_units + 1;
    }
    out
}

/// `12. ` at the start: how many characters it takes.
fn ordered_marker(chars: &[char]) -> Option<usize> {
    let digits = chars.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && chars.get(digits) == Some(&'.') && chars.get(digits + 1) == Some(&' ') {
        Some(digits + 2)
    } else {
        None
    }
}

fn find(chars: &[char], from: usize, pat: &[char]) -> Option<usize> {
    if pat.is_empty() || chars.len() < pat.len() {
        return None;
    }
    (from..=chars.len() - pat.len()).find(|&i| chars[i..i + pat.len()] == *pat)
}

fn inline(
    chars: &[char],
    from: usize,
    span: &dyn Fn(usize, usize, Kind) -> Span,
    out: &mut Vec<Span>,
) {
    let mut i = from;
    while i < chars.len() {
        let c = chars[i];
        // Image: ![alt](url)
        if c == '!' && chars.get(i + 1) == Some(&'[') {
            if let Some(close) = find(chars, i + 2, &[']', '(']) {
                if let Some(end) = find(chars, close + 2, &[')']) {
                    out.push(span(i, end + 1, Kind::Image));
                    i = end + 1;
                    continue;
                }
            }
        }
        // Inline code.
        if c == '`' {
            if let Some(end) = find(chars, i + 1, &['`']) {
                out.push(span(i, i + 1, Kind::Marker));
                if end > i + 1 {
                    out.push(span(i + 1, end, Kind::Code));
                }
                out.push(span(end, end + 1, Kind::Marker));
                i = end + 1;
                continue;
            }
        }
        // Bold, strike: doubled markers.
        if (c == '*' || c == '_' || c == '~') && chars.get(i + 1) == Some(&c) {
            if let Some(end) = find(chars, i + 2, &[c, c]) {
                if end > i + 2 {
                    out.push(span(i, i + 2, Kind::Marker));
                    out.push(span(
                        i + 2,
                        end,
                        if c == '~' { Kind::Strike } else { Kind::Bold },
                    ));
                    out.push(span(end, end + 2, Kind::Marker));
                    i = end + 2;
                    continue;
                }
            }
        }
        // Italic: a single marker, not at a word's middle for `_`.
        if (c == '*' || c == '_') && chars.get(i + 1).is_some_and(|n| *n != c && *n != ' ') {
            let word_start = i == 0 || !chars[i - 1].is_alphanumeric();
            if c == '*' || word_start {
                if let Some(end) = find(chars, i + 1, &[c]) {
                    if end > i + 1 && chars[end - 1] != ' ' {
                        out.push(span(i, i + 1, Kind::Marker));
                        out.push(span(i + 1, end, Kind::Italic));
                        out.push(span(end, end + 1, Kind::Marker));
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        // #tag
        if c == '#' && (i == 0 || chars[i - 1] == ' ' || chars[i - 1] == '(' || chars[i - 1] == '\t') {
            let n = chars[i + 1..].iter().take_while(|c| is_tag_char(**c)).count();
            if n > 0 {
                let name: String = chars[i + 1..i + 1 + n].iter().collect();
                out.push(span(i, i + 1 + n, Kind::Tag(name)));
                i += 1 + n;
                continue;
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(usize, usize, Kind)> {
        parse(text)
            .into_iter()
            .map(|s| (s.start, s.end, s.kind))
            .collect()
    }

    #[test]
    fn heading_marker_and_text() {
        assert_eq!(
            kinds("# План"),
            vec![(0, 2, Kind::Marker), (2, 6, Kind::Heading(1))]
        );
    }

    #[test]
    fn checklist_boxes_and_done_text() {
        let k = kinds("- [ ] a\n- [x] b");
        assert!(k.contains(&(0, 6, Kind::Box { checked: false })));
        assert!(k.contains(&(8, 14, Kind::Box { checked: true })));
        assert!(k.contains(&(14, 15, Kind::Done)));
    }

    #[test]
    fn bold_italic_code_and_tags() {
        let k = kinds("a **b** *c* `d` #тег");
        assert!(k.contains(&(4, 5, Kind::Bold)));
        assert!(k.contains(&(9, 10, Kind::Italic)));
        assert!(k.contains(&(13, 14, Kind::Code)));
        assert!(k.contains(&(16, 20, Kind::Tag("тег".into()))));
    }

    #[test]
    fn a_hash_inside_a_word_is_no_tag() {
        assert!(!kinds("a#b").iter().any(|(_, _, k)| matches!(k, Kind::Tag(_))));
    }

    #[test]
    fn fenced_code_lines() {
        let k = kinds("```\nlet a = 1\n```");
        assert!(k.contains(&(4, 13, Kind::CodeBlock)));
    }

    #[test]
    fn offsets_count_utf16() {
        // «😀» is two UTF-16 units.
        let k = kinds("😀 #т");
        assert!(k.contains(&(3, 5, Kind::Tag("т".into()))));
    }
}
