//! The Markdown agents write, as blocks of styled text.
//!
//! This covers what coding agents actually produce: paragraphs, headings,
//! lists, quotes, fenced code, tables, rules, and inline bold, italic, code,
//! and links. Anything else stays as literal text, so nothing an agent writes
//! is ever lost, only shown plainly.

/// How a run of text is styled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: bool,
}

/// A run of text in one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: SpanStyle,
}

/// One block of a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Span>),
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    ListItem {
        /// The item's number, for an ordered list.
        number: Option<u64>,
        /// Nesting depth, from zero.
        depth: u8,
        /// A task-list checkbox, when present.
        checked: Option<bool>,
        spans: Vec<Span>,
    },
    Quote(Vec<Span>),
    Code {
        language: String,
        text: String,
    },
    /// A table, kept as aligned monospaced rows.
    Table(Vec<String>),
    Rule,
}

/// Splits `text` into blocks.
pub fn parse(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut lines = text.lines().peekable();

    let flush = |paragraph: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(inline(&paragraph.join("\n"))));
            paragraph.clear();
        }
    };

    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if let Some(fence) = code_fence(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let indent = line.len() - trimmed.len();
            let language = trimmed[fence.len()..].trim().to_owned();
            let mut code = Vec::new();
            for code_line in lines.by_ref() {
                let candidate = code_line.trim_start();
                if candidate.starts_with(fence)
                    && candidate[fence.len()..].trim().is_empty()
                    && candidate
                        .chars()
                        .take_while(|c| *c == fence_char(fence))
                        .count()
                        >= fence.len()
                {
                    break;
                }
                code.push(strip_indent(code_line, indent));
            }
            blocks.push(Block::Code {
                language,
                text: code.join("\n"),
            });
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
            continue;
        }
        if is_rule(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
            continue;
        }
        if let Some((level, heading)) = heading(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading {
                level,
                spans: inline(heading),
            });
            continue;
        }
        if trimmed.starts_with('|') && lines.peek().is_some_and(|next| is_table_divider(next)) {
            flush(&mut paragraph, &mut blocks);
            let mut rows = vec![table_cells(trimmed)];
            lines.next();
            while let Some(next) = lines.peek() {
                let next = next.trim();
                if !next.starts_with('|') {
                    break;
                }
                rows.push(table_cells(next));
                lines.next();
            }
            blocks.push(Block::Table(align_table(&rows)));
            continue;
        }
        if let Some(quote) = trimmed.strip_prefix('>') {
            flush(&mut paragraph, &mut blocks);
            let mut quoted = vec![quote.trim_start()];
            while let Some(next) = lines.peek() {
                let Some(more) = next.trim_start().strip_prefix('>') else {
                    break;
                };
                quoted.push(more.trim_start());
                lines.next();
            }
            blocks.push(Block::Quote(inline(&quoted.join("\n"))));
            continue;
        }
        if let Some((number, body)) = list_item(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let depth = (indent_width(line) / 2).min(6) as u8;
            let (checked, body) = task_box(body);
            let mut text = body.to_owned();
            // Lazy continuation lines belong to the item.
            while let Some(next) = lines.peek() {
                let next_trimmed = next.trim_start();
                if next_trimmed.is_empty()
                    || list_item(next_trimmed).is_some()
                    || code_fence(next_trimmed).is_some()
                    || heading(next_trimmed).is_some()
                    || next_trimmed.starts_with('>')
                    || indent_width(next) == 0
                {
                    break;
                }
                text.push('\n');
                text.push_str(next_trimmed);
                lines.next();
            }
            blocks.push(Block::ListItem {
                number,
                depth,
                checked,
                spans: inline(&text),
            });
            continue;
        }
        paragraph.push(trimmed);
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

fn code_fence(line: &str) -> Option<&str> {
    ["````", "```", "~~~~", "~~~"]
        .into_iter()
        .find(|fence| line.starts_with(fence))
        .map(|fence| {
            let character = fence_char(fence);
            let length = line.chars().take_while(|c| *c == character).count();
            &line[..length]
        })
}

fn fence_char(fence: &str) -> char {
    fence.chars().next().unwrap_or('`')
}

fn strip_indent(line: &str, indent: usize) -> &str {
    let removable = line
        .char_indices()
        .take_while(|(index, character)| *index < indent && *character == ' ')
        .count();
    &line[removable..]
}

fn indent_width(line: &str) -> usize {
    line.chars()
        .take_while(|character| character.is_whitespace())
        .map(|character| if character == '\t' { 4 } else { 1 })
        .sum()
}

fn is_rule(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|marker| compact.chars().all(|c| c == *marker))
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &line[level..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some((level as u8, rest.trim().trim_end_matches('#').trim_end()))
}

fn list_item(line: &str) -> Option<(Option<u64>, &str)> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(body) = line.strip_prefix(marker) {
            return Some((None, body));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let rest = &line[digits..];
    let body = rest
        .strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))?;
    Some((line[..digits].parse().ok(), body))
}

fn task_box(body: &str) -> (Option<bool>, &str) {
    if let Some(rest) = body.strip_prefix("[ ] ") {
        (Some(false), rest)
    } else if let Some(rest) = body
        .strip_prefix("[x] ")
        .or_else(|| body.strip_prefix("[X] "))
    {
        (Some(true), rest)
    } else {
        (None, body)
    }
}

fn is_table_divider(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('|')
        && line.contains('-')
        && line
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

fn table_cells(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    inner
        .split('|')
        .map(|cell| plain_text(&inline(cell.trim())))
        .collect()
}

fn align_table(rows: &[Vec<String>]) -> Vec<String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut lines = Vec::with_capacity(rows.len() + 1);
    for (index, row) in rows.iter().enumerate() {
        let cells: Vec<String> = (0..columns)
            .map(|column| {
                let cell = row.get(column).map_or("", String::as_str);
                let padding = widths[column].saturating_sub(cell.chars().count());
                format!("{cell}{}", " ".repeat(padding))
            })
            .collect();
        lines.push(cells.join("  ").trim_end().to_owned());
        if index == 0 {
            lines.push(
                widths
                    .iter()
                    .map(|width| "─".repeat(*width))
                    .collect::<Vec<_>>()
                    .join("  "),
            );
        }
    }
    lines
}

/// The text of spans without their styling.
pub fn plain_text(spans: &[Span]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

/// Styled runs for one block's text.
pub fn inline(text: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut style = SpanStyle::default();
    let mut current = String::new();
    let characters: Vec<char> = text.chars().collect();
    let mut index = 0;

    let push = |spans: &mut Vec<Span>, current: &mut String, style: SpanStyle| {
        if current.is_empty() {
            return;
        }
        match spans.last_mut() {
            Some(last) if last.style == style => last.text.push_str(current),
            _ => spans.push(Span {
                text: current.clone(),
                style,
            }),
        }
        current.clear();
    };

    while index < characters.len() {
        let character = characters[index];
        match character {
            '\\' if characters
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_punctuation()) =>
            {
                current.push(characters[index + 1]);
                index += 2;
            }
            '`' => {
                let run = run_length(&characters, index, '`');
                if let Some(end) = find_run(&characters, index + run, '`', run) {
                    push(&mut spans, &mut current, style);
                    let code: String = characters[index + run..end].iter().collect();
                    let code = if code.starts_with(' ') && code.ends_with(' ') && code.len() > 2 {
                        code[1..code.len() - 1].to_owned()
                    } else {
                        code
                    };
                    spans.push(Span {
                        text: code,
                        style: SpanStyle {
                            code: true,
                            ..style
                        },
                    });
                    index = end + run;
                } else {
                    current.extend(&characters[index..index + run]);
                    index += run;
                }
            }
            '*' | '_' => {
                let run = run_length(&characters, index, character);
                let previous = index.checked_sub(1).map(|i| characters[i]);
                let next = characters.get(index + run).copied();
                // An underscore inside a word (snake_case) is never emphasis.
                let intraword = character == '_'
                    && previous.is_some_and(char::is_alphanumeric)
                    && next.is_some_and(char::is_alphanumeric);
                let opens = next.is_some_and(|c| !c.is_whitespace()) && !intraword;
                let closes = previous.is_some_and(|c| !c.is_whitespace()) && !intraword;
                let toggles = |active: bool| (active && closes) || (!active && opens);
                match run {
                    1 if toggles(style.italic)
                        && (style.italic
                            || find_closing(&characters, index + 1, character, 1).is_some()) =>
                    {
                        push(&mut spans, &mut current, style);
                        style.italic = !style.italic;
                    }
                    2 if toggles(style.bold)
                        && (style.bold
                            || find_closing(&characters, index + 2, character, 2).is_some()) =>
                    {
                        push(&mut spans, &mut current, style);
                        style.bold = !style.bold;
                    }
                    3 if toggles(style.bold && style.italic)
                        && ((style.bold && style.italic)
                            || find_closing(&characters, index + 3, character, 3).is_some()) =>
                    {
                        push(&mut spans, &mut current, style);
                        style.bold = !style.bold;
                        style.italic = !style.italic;
                    }
                    _ => current.extend(&characters[index..index + run]),
                }
                index += run;
            }
            '[' => {
                if let Some((label_end, url_end)) = link_at(&characters, index) {
                    push(&mut spans, &mut current, style);
                    let label: String = characters[index + 1..label_end].iter().collect();
                    for span in inline(&label) {
                        spans.push(Span {
                            text: span.text,
                            style: SpanStyle {
                                link: true,
                                bold: span.style.bold || style.bold,
                                italic: span.style.italic || style.italic,
                                code: span.style.code,
                            },
                        });
                    }
                    index = url_end + 1;
                } else {
                    current.push(character);
                    index += 1;
                }
            }
            _ => {
                current.push(character);
                index += 1;
            }
        }
    }
    push(&mut spans, &mut current, style);
    spans
}

fn run_length(characters: &[char], start: usize, character: char) -> usize {
    characters[start..]
        .iter()
        .take_while(|c| **c == character)
        .count()
}

fn find_run(characters: &[char], from: usize, character: char, length: usize) -> Option<usize> {
    let mut index = from;
    while index < characters.len() {
        if characters[index] == character {
            let run = run_length(characters, index, character);
            if run == length {
                return Some(index);
            }
            index += run;
        } else {
            index += 1;
        }
    }
    None
}

/// A later delimiter run that could close emphasis opened at `from`.
fn find_closing(characters: &[char], from: usize, character: char, length: usize) -> Option<usize> {
    let mut index = from;
    while index < characters.len() {
        if characters[index] == '`' {
            let run = run_length(characters, index, '`');
            index =
                find_run(characters, index + run, '`', run).map_or(index + run, |end| end + run);
            continue;
        }
        if characters[index] == character {
            let run = run_length(characters, index, character);
            let previous = characters[index - 1];
            if run >= length && !previous.is_whitespace() && index > from {
                return Some(index);
            }
            index += run;
        } else {
            index += 1;
        }
    }
    None
}

/// `[label](url)` starting at `start`: the label's closing bracket and the
/// url's closing parenthesis.
fn link_at(characters: &[char], start: usize) -> Option<(usize, usize)> {
    let mut depth = 0;
    let mut label_end = None;
    for (offset, character) in characters[start..].iter().enumerate() {
        match character {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    label_end = Some(start + offset);
                    break;
                }
            }
            '\n' => return None,
            _ => {}
        }
    }
    let label_end = label_end?;
    if characters.get(label_end + 1) != Some(&'(') {
        return None;
    }
    let url_end = characters[label_end + 2..]
        .iter()
        .position(|character| *character == ')' || character.is_whitespace())
        .map(|offset| label_end + 2 + offset)?;
    (characters[url_end] == ')' && url_end > label_end + 2).then_some((label_end, url_end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Span {
        Span {
            text: text.into(),
            style: SpanStyle::default(),
        }
    }

    fn styled(text: &str, style: SpanStyle) -> Span {
        Span {
            text: text.into(),
            style,
        }
    }

    const BOLD: SpanStyle = SpanStyle {
        bold: true,
        italic: false,
        code: false,
        link: false,
    };
    const ITALIC: SpanStyle = SpanStyle {
        bold: false,
        italic: true,
        code: false,
        link: false,
    };
    const CODE: SpanStyle = SpanStyle {
        bold: false,
        italic: false,
        code: true,
        link: false,
    };
    const LINK: SpanStyle = SpanStyle {
        bold: false,
        italic: false,
        code: false,
        link: true,
    };

    #[test]
    fn inline_styles_nest_and_code_is_literal() {
        assert_eq!(
            inline("A **bold** and *italic* `x_*y*` [docs](https://a.b/c)."),
            vec![
                plain("A "),
                styled("bold", BOLD),
                plain(" and "),
                styled("italic", ITALIC),
                plain(" "),
                styled("x_*y*", CODE),
                plain(" "),
                styled("docs", LINK),
                plain("."),
            ]
        );
    }

    #[test]
    fn identifiers_and_lone_markers_stay_literal() {
        assert_eq!(
            inline("snake_case_name and 2 * 3 and a*b"),
            vec![plain("snake_case_name and 2 * 3 and a*b")]
        );
        assert_eq!(inline("unclosed **bold"), vec![plain("unclosed **bold")]);
        assert_eq!(inline("`unclosed code"), vec![plain("`unclosed code")]);
        assert_eq!(inline(r"\*not italic\*"), vec![plain("*not italic*")]);
        assert_eq!(inline("[not a link] (x)"), vec![plain("[not a link] (x)")]);
        assert_eq!(inline("a__b__c"), vec![plain("a__b__c")]);
        // As in CommonMark, flanking underscores are emphasis.
        assert_eq!(
            inline("__init__ and __bold__"),
            vec![styled("init", BOLD), plain(" and "), styled("bold", BOLD)]
        );
    }

    #[test]
    fn blocks_cover_what_agents_write() {
        let text = "\
# Plan

Some *text*
continues here.

1. First
2. Second
   still second
- [x] done
  - nested

> quoted
> more

```rust
fn main() {}
```

---
| Name | Size |
|------|-----:|
| a.rs | `10` |
";
        let blocks = parse(text);
        assert_eq!(
            blocks[0],
            Block::Heading {
                level: 1,
                spans: vec![plain("Plan")]
            }
        );
        assert_eq!(
            blocks[1],
            Block::Paragraph(vec![
                plain("Some "),
                styled("text", ITALIC),
                plain("\ncontinues here.")
            ])
        );
        assert_eq!(
            blocks[2],
            Block::ListItem {
                number: Some(1),
                depth: 0,
                checked: None,
                spans: vec![plain("First")]
            }
        );
        assert_eq!(
            blocks[3],
            Block::ListItem {
                number: Some(2),
                depth: 0,
                checked: None,
                spans: vec![plain("Second\nstill second")]
            }
        );
        assert_eq!(
            blocks[4],
            Block::ListItem {
                number: None,
                depth: 0,
                checked: Some(true),
                spans: vec![plain("done")]
            }
        );
        assert_eq!(
            blocks[5],
            Block::ListItem {
                number: None,
                depth: 1,
                checked: None,
                spans: vec![plain("nested")]
            }
        );
        assert_eq!(blocks[6], Block::Quote(vec![plain("quoted\nmore")]));
        assert_eq!(
            blocks[7],
            Block::Code {
                language: "rust".into(),
                text: "fn main() {}".into()
            }
        );
        assert_eq!(blocks[8], Block::Rule);
        assert_eq!(
            blocks[9],
            Block::Table(vec![
                "Name  Size".into(),
                "────  ────".into(),
                "a.rs  10".into()
            ])
        );
        assert_eq!(blocks.len(), 10);
    }

    #[test]
    fn code_blocks_keep_everything_until_a_matching_fence() {
        let blocks = parse("````md\n```\ninner\n```\n````\nafter");
        assert_eq!(
            blocks[0],
            Block::Code {
                language: "md".into(),
                text: "```\ninner\n```".into()
            }
        );
        assert_eq!(blocks[1], Block::Paragraph(vec![plain("after")]));
        // An unclosed fence runs to the end, as it does while streaming.
        assert_eq!(
            parse("```\nstill typing")[0],
            Block::Code {
                language: String::new(),
                text: "still typing".into()
            }
        );
    }

    #[test]
    fn headings_need_a_space_and_rules_need_three_markers() {
        assert_eq!(
            parse("#hashtag")[0],
            Block::Paragraph(vec![plain("#hashtag")])
        );
        assert_eq!(
            parse("## Title ##")[0],
            Block::Heading {
                level: 2,
                spans: vec![plain("Title")]
            }
        );
        assert_eq!(parse("--")[0], Block::Paragraph(vec![plain("--")]));
        assert_eq!(parse("* * *")[0], Block::Rule);
    }
}
