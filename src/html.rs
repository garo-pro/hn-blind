//! Converts the small, predictable subset of HTML that HN puts in comment and
//! self-post bodies into plain text.
//!
//! HN emits `<p>` (unclosed) for paragraph breaks, `<i>`, `<b>`, `<code>`,
//! `<pre>`, and `<a href="...">` links whose text is usually the URL itself. It
//! escapes `& < > " '` and slashes as entities. Nothing here needs a real HTML
//! parser, and a screen reader should never be handed raw markup.

/// Convert an HN HTML fragment to plain text suitable for speech and for an
/// accessibility node label.
pub fn to_text(html: &str) -> String {
    let stripped = strip_tags(html);
    let decoded = decode_entities(&stripped);
    normalize_whitespace(&decoded)
}

/// Append literal text, folding source newlines to spaces.
///
/// In HTML a raw newline is ordinary whitespace, not a line break. Folding it
/// here means the only newlines downstream are the ones tags introduced, so
/// paragraph structure survives whitespace normalization.
fn push_text(out: &mut String, text: &str) {
    for c in text.chars() {
        out.push(if c == '\n' || c == '\r' { ' ' } else { c });
    }
}

/// Remove tags, turning paragraph and line-break tags into newlines.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(open) = rest.find('<') {
        push_text(&mut out, &rest[..open]);
        let after = &rest[open + 1..];

        // A '<' with no matching '>' is literal text, not a tag.
        let Some(close) = after.find('>') else {
            push_text(&mut out, &rest[open..]);
            return out;
        };

        let tag = &after[..close];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect();

        // Block-level boundaries carry meaning for a listener; inline tags do not.
        match name.as_str() {
            "p" | "br" | "div" | "li" | "tr" => out.push_str("\n\n"),
            "pre" | "blockquote" => out.push('\n'),
            _ => {}
        }

        rest = &after[close + 1..];
    }

    push_text(&mut out, rest);
    out
}

/// Decode the HTML entities HN produces, both named and numeric.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];

        // Entities are short; a distant ';' means this '&' was literal.
        let end = after.chars().take(12).position(|c| c == ';');
        let Some(end) = end else {
            out.push('&');
            rest = after;
            continue;
        };

        let body = &after[..end];
        match resolve_entity(body) {
            Some(c) => out.push(c),
            None => {
                // Unknown entity: keep it verbatim rather than silently dropping it.
                out.push('&');
                out.push_str(body);
                out.push(';');
            }
        }
        rest = &after[end + 1..];
    }

    out.push_str(rest);
    out
}

fn resolve_entity(body: &str) -> Option<char> {
    if let Some(digits) = body.strip_prefix('#') {
        let code = match digits.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => digits.parse::<u32>().ok()?,
        };
        return char::from_u32(code);
    }

    Some(match body {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "rsquo" => '\u{2019}',
        "lsquo" => '\u{2018}',
        "ldquo" => '\u{201C}',
        "rdquo" => '\u{201D}',
        _ => return None,
    })
}

/// Collapse runs of spaces and blank lines without destroying paragraph breaks.
fn normalize_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_newlines = 0usize;
    let mut pending_space = false;
    let mut wrote_any = false;

    for c in text.chars() {
        match c {
            '\n' | '\r' => pending_newlines += 1,
            c if c.is_whitespace() => pending_space = true,
            c => {
                if wrote_any {
                    if pending_newlines >= 2 {
                        out.push_str("\n\n");
                    } else if pending_newlines == 1 {
                        out.push('\n');
                    } else if pending_space {
                        out.push(' ');
                    }
                }
                out.push(c);
                wrote_any = true;
                pending_newlines = 0;
                pending_space = false;
            }
        }
    }

    out
}

/// Flatten text to a single line, for use as a list-item label where newlines
/// would be meaningless.
pub fn single_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
        } else {
            if space {
                out.push(' ');
            }
            out.push(c);
            space = false;
        }
    }
    out
}

/// Truncate on a character boundary, appending an ellipsis when shortened.
pub fn truncate(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => format!("{}…", text[..idx].trim_end()),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_become_blank_lines() {
        assert_eq!(to_text("one<p>two"), "one\n\ntwo");
    }

    #[test]
    fn inline_tags_are_stripped_without_adding_space() {
        assert_eq!(to_text("a<i>b</i>c"), "abc");
    }

    #[test]
    fn links_keep_their_visible_text() {
        assert_eq!(
            to_text(r#"see <a href="https://x.test" rel="nofollow">https://x.test</a>"#),
            "see https://x.test"
        );
    }

    #[test]
    fn named_and_numeric_entities_decode() {
        assert_eq!(to_text("a &amp; b"), "a & b");
        assert_eq!(to_text("don&#x27;t"), "don't");
        assert_eq!(to_text("don&#39;t"), "don't");
        assert_eq!(to_text("1 &lt; 2 &gt; 0"), "1 < 2 > 0");
        assert_eq!(to_text("a&#x2F;b"), "a/b");
    }

    #[test]
    fn stray_ampersand_and_lt_survive() {
        assert_eq!(to_text("Tom & Jerry"), "Tom & Jerry");
        assert_eq!(to_text("a &notarealentity; b"), "a &notarealentity; b");
        assert_eq!(to_text("5 < x"), "5 < x");
    }

    #[test]
    fn whitespace_runs_collapse_but_paragraphs_remain() {
        assert_eq!(to_text("a   \n  b<p>  c  "), "a b\n\nc");
    }

    #[test]
    fn single_line_flattens_paragraphs() {
        assert_eq!(single_line("a\n\nb  c"), "a b c");
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("abcdef", 3), "abc…");
        assert_eq!(truncate("abc", 10), "abc");
        // Multi-byte characters must not be split mid-codepoint.
        assert_eq!(truncate("héllo wörld", 4), "héll…");
    }
}
