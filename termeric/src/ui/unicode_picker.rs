//! Bounded Unicode character selection built on the shared palette component.

mod annotations;

use self::annotations::{Annotation, CLDR_LOCALE, CLDR_VERSION};
use crate::ui::command_palette::palette::{CommandSource, RegisteredCommand};
use crate::ui::command_palette::{CommandPalette, PaletteInfo, PaletteLayout, PaletteResult};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::mem::size_of;
use std::sync::{Once, OnceLock};

const UNICODE_DATA_VERSION: &str = "17.0";
const UNICODE_SEARCH_RESULT_LIMIT: usize = 256;
const UNICODE_SEARCH_MEMORY_BUDGET: usize = 8 * 1024 * 1024;
const UNICODE_RESULT_MEMORY_BUDGET: usize = 1024 * 1024;
const UNICODE_NAME_BYTES_CAPACITY: usize = 4_000_000;
const UNICODE_RECORD_CAPACITY: usize = 160_000;

static UNICODE_NAME_INDEX: OnceLock<UnicodeNameIndex> = OnceLock::new();
static UNICODE_PREWARM: Once = Once::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnicodePickerSelection {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy)]
struct CuratedCharacter {
    text: &'static str,
    name: &'static str,
    category: &'static str,
    aliases: &'static str,
}

pub struct UnicodePicker {
    palette: CommandPalette,
    actions: Vec<UnicodePickerSelection>,
}

#[derive(Debug, Clone, Copy)]
struct UnicodeNameRecord {
    scalar: u32,
    name_start: u32,
}

struct UnicodeNameIndex {
    names: String,
    records: Vec<UnicodeNameRecord>,
}

impl UnicodeNameIndex {
    fn build() -> Self {
        let mut names = String::with_capacity(UNICODE_NAME_BYTES_CAPACITY);
        let mut records = Vec::with_capacity(UNICODE_RECORD_CAPACITY);
        for scalar in 0..=char::MAX as u32 {
            let Some(character) = char::from_u32(scalar) else {
                continue;
            };
            if character.is_control() {
                continue;
            }
            let Some(name) = unicode_names2::name(character) else {
                continue;
            };
            let name = name.to_string();
            let name_start =
                u32::try_from(names.len()).expect("Unicode name index exceeded four GiB");
            names.push_str(&name);
            records.push(UnicodeNameRecord { scalar, name_start });
        }
        assert!(names.len() <= UNICODE_NAME_BYTES_CAPACITY);
        assert!(records.len() <= UNICODE_RECORD_CAPACITY);
        names.shrink_to_fit();
        records.shrink_to_fit();
        let index = Self { names, records };
        assert!(index.retained_bytes() <= UNICODE_SEARCH_MEMORY_BUDGET);
        index
    }

    fn name(&self, position: usize) -> &str {
        let start = self.records[position].name_start as usize;
        let end = self
            .records
            .get(position + 1)
            .map_or(self.names.len(), |record| record.name_start as usize);
        &self.names[start..end]
    }

    fn retained_bytes(&self) -> usize {
        self.names.capacity() + self.records.capacity() * std::mem::size_of::<UnicodeNameRecord>()
    }
}

#[derive(Debug)]
struct RankedCharacter {
    score: u32,
    tie_break: String,
    selection: UnicodePickerSelection,
    command: RegisteredCommand,
}

impl UnicodePicker {
    pub fn new() -> Self {
        let mut palette = CommandPalette::new();
        palette.set_presentation("Search characters or enter U+2192...", "character", "Close");
        Self {
            palette,
            actions: Vec::new(),
        }
    }

    pub fn open(&mut self) {
        self.rebuild_registry();
        self.palette.open();
        prewarm_unicode_name_index();
    }

    pub fn close(&mut self) {
        self.palette.close();
        self.actions.clear();
    }

    pub fn is_active(&self) -> bool {
        self.palette.is_active()
    }

    pub fn update_animation(&mut self) -> bool {
        self.palette.update_animation()
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.palette.set_reduced_motion(reduced_motion);
    }

    pub fn build_info(&self) -> PaletteInfo {
        self.palette.build_info()
    }

    pub fn command_ids(&self) -> impl Iterator<Item = &str> {
        self.palette.command_ids()
    }

    pub fn set_max_visible(&mut self, max_visible: usize) {
        self.palette.set_max_visible(max_visible);
    }

    pub fn select_prev(&mut self) {
        self.palette.select_prev();
    }

    pub fn select_next(&mut self) {
        self.palette.select_next();
    }

    pub fn scroll_up(&mut self, amount: usize) {
        self.palette.scroll_up(amount);
    }

    pub fn scroll_down(&mut self, amount: usize) {
        self.palette.scroll_down(amount);
    }

    pub fn handle_char(&mut self, character: char) {
        self.palette.handle_char(character);
        self.refresh_dynamic_entry();
    }

    pub fn handle_backspace(&mut self) {
        self.palette.handle_backspace();
        self.refresh_dynamic_entry();
    }

    pub fn accessibility_set_filter(&mut self, value: &str) -> bool {
        if !self.palette.accessibility_set_input(value) {
            return false;
        }
        self.refresh_dynamic_entry();
        true
    }

    pub fn select_command(&mut self, id: &str) -> bool {
        self.palette.select_command(id)
    }

    pub fn activate(&mut self) -> Option<UnicodePickerSelection> {
        let result = self.palette.execute();
        self.resolve(result)
    }

    pub fn execute_command(&mut self, id: &str) -> Option<UnicodePickerSelection> {
        let result = self.palette.execute_command(id)?;
        self.resolve(result)
    }

    pub fn handle_click(
        &mut self,
        x: f32,
        y: f32,
        layout: &PaletteLayout,
    ) -> Option<UnicodePickerSelection> {
        let result = self.palette.handle_click(x, y, layout)?;
        self.resolve(result)
    }

    fn refresh_dynamic_entry(&mut self) {
        let input = self.palette.build_info().input;
        if looks_like_scalar_query(&input) {
            if let Some(character) = parse_scalar_query(&input) {
                self.rebuild_exact_registry(character);
            } else {
                self.actions.clear();
                self.palette.replace_all_commands(Vec::new());
            }
        } else if input.trim().is_empty() {
            self.rebuild_registry();
        } else {
            self.rebuild_search_registry(&input);
        }
    }

    fn rebuild_registry(&mut self) {
        self.actions.clear();
        let mut commands = Vec::with_capacity(CURATED_CHARACTERS.len());
        for entry in CURATED_CHARACTERS {
            let id = stable_character_id("curated", entry.text);
            self.actions.push(UnicodePickerSelection {
                id: id.clone(),
                text: entry.text.into(),
            });
            commands.push(RegisteredCommand {
                id,
                title: format!("{}  {}", entry.text, entry.name),
                category: format!(
                    "{} · {} · {}",
                    entry.category,
                    display_codepoints(entry.text),
                    entry.aliases
                ),
                shortcut: None,
                icon: None,
                source: CommandSource::Core,
                destructive: false,
            });
        }
        self.palette.replace_all_commands(commands);
    }

    fn rebuild_exact_registry(&mut self, character: char) {
        self.actions.clear();
        let text = character.to_string();
        let curated = CURATED_CHARACTERS.iter().find(|entry| entry.text == text);
        let (kind, title, category) = curated.map_or_else(
            || {
                (
                    "codepoint",
                    format!("{text}  {}", display_codepoints(&text)),
                    "Exact code point".to_string(),
                )
            },
            |entry| {
                (
                    "curated",
                    format!("{}  {}", entry.text, entry.name),
                    format!("{} · {}", entry.category, entry.aliases),
                )
            },
        );
        let id = stable_character_id(kind, &text);
        self.actions.push(UnicodePickerSelection {
            id: id.clone(),
            text,
        });
        self.palette.replace_all_commands(vec![RegisteredCommand {
            id,
            title,
            category: format!(
                "{category} · {} · 0x{:X} · {:X}",
                display_codepoints(&character.to_string()),
                character as u32,
                character as u32
            ),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        }]);
    }

    fn rebuild_search_registry(&mut self, query: &str) {
        let results = search_unicode_characters(query);
        self.actions = results
            .iter()
            .map(|result| result.selection.clone())
            .collect();
        self.palette
            .replace_ranked_results(results.into_iter().map(|result| result.command).collect());
    }

    fn resolve(&mut self, result: PaletteResult) -> Option<UnicodePickerSelection> {
        let PaletteResult::RunAction(id) = result else {
            if matches!(result, PaletteResult::Cancelled) {
                self.close();
            }
            return None;
        };
        let selection = self.actions.iter().find(|action| action.id == id).cloned();
        if selection.is_none() {
            self.close();
        }
        selection
    }
}

fn prewarm_unicode_name_index() {
    UNICODE_PREWARM.call_once(|| {
        std::thread::spawn(|| {
            UNICODE_NAME_INDEX.get_or_init(UnicodeNameIndex::build);
            annotations::index();
        });
    });
}

fn unicode_name_index() -> &'static UnicodeNameIndex {
    UNICODE_NAME_INDEX.get_or_init(UnicodeNameIndex::build)
}

fn search_unicode_characters(query: &str) -> Vec<RankedCharacter> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }

    let normalized = normalize_name_query(query);
    let terms = normalized.split_whitespace().collect::<Vec<_>>();
    let exact_scalar = (!normalized.is_empty())
        .then(|| unicode_names2::character(query))
        .flatten()
        .filter(|character| !character.is_control())
        .map(|character| character as u32);

    // The heap retains only the best scalar candidates. Its natural rank is
    // `(score, lower-code-point-first)`, wrapped in Reverse so peek() is the
    // current worst result.
    let mut best = BinaryHeap::<Reverse<(u32, Reverse<u32>)>>::new();
    let mut exact_matched = false;
    if !terms.is_empty() {
        let index = unicode_name_index();
        for (position, record) in index.records.iter().enumerate() {
            let name = index.name(position);
            let Some(mut score) = score_unicode_name(name, &terms) else {
                continue;
            };
            if exact_scalar == Some(record.scalar) {
                score = score.max(4_000_000);
                exact_matched = true;
            }
            let rank = (score, Reverse(record.scalar));
            retain_rank(&mut best, rank);
        }
    }

    // Some aliases share no words with the canonical name (for example,
    // BYTE ORDER MARK -> ZERO WIDTH NO-BREAK SPACE), so inject a safe exact
    // alias/name resolution independently of partial canonical-name matching.
    if let Some(scalar) = exact_scalar.filter(|_| !exact_matched) {
        retain_rank(&mut best, (4_000_000, Reverse(scalar)));
    }

    let index = unicode_name_index();
    let mut ranked = best
        .into_iter()
        .filter_map(|Reverse((score, Reverse(scalar)))| {
            let character = char::from_u32(scalar)?;
            let position = index
                .records
                .binary_search_by_key(&scalar, |record| record.scalar)
                .ok()?;
            let name = index.name(position);
            Some(named_result(character, name, score))
        })
        .collect::<Vec<_>>();

    if !terms.is_empty() {
        let annotations = annotations::index();
        assert!(annotations.static_bytes() <= annotations::CLDR_STATIC_BYTES_BUDGET);
        let mut best = BinaryHeap::<Reverse<(u32, Reverse<u32>)>>::new();
        for position in 0..annotations.len() {
            let annotation = annotations.annotation(position);
            let Some(score) = score_cldr_annotation(annotation, query, &terms) else {
                continue;
            };
            retain_rank(
                &mut best,
                (
                    score,
                    Reverse(u32::try_from(position).expect("CLDR index exceeded u32")),
                ),
            );
        }
        ranked.extend(best.into_iter().map(|Reverse((score, Reverse(position)))| {
            cldr_result(annotations.annotation(position as usize), score)
        }));
    }

    for (position, entry) in CURATED_CHARACTERS.iter().enumerate() {
        if let Some(score) = score_curated_character(entry, query, &terms) {
            ranked.push(curated_result(entry, score, position));
        }
    }

    ranked.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.tie_break.cmp(&b.tie_break))
    });
    let mut seen = HashSet::with_capacity(UNICODE_SEARCH_RESULT_LIMIT);
    ranked.retain(|result| seen.insert(result.selection.text.clone()));
    ranked.truncate(UNICODE_SEARCH_RESULT_LIMIT);
    ranked.shrink_to_fit();
    assert!(ranked_results_owned_bytes(&ranked) <= UNICODE_RESULT_MEMORY_BUDGET);
    ranked
}

fn retain_rank(best: &mut BinaryHeap<Reverse<(u32, Reverse<u32>)>>, rank: (u32, Reverse<u32>)) {
    if best.len() < UNICODE_SEARCH_RESULT_LIMIT {
        best.push(Reverse(rank));
    } else if best.peek().is_some_and(|worst| rank > worst.0) {
        best.pop();
        best.push(Reverse(rank));
    }
}

fn normalize_name_query(query: &str) -> String {
    let mut normalized = String::with_capacity(query.len());
    let mut separator = false;
    for character in query.chars() {
        if character.is_alphanumeric() {
            if separator && !normalized.is_empty() {
                normalized.push(' ');
            }
            normalized.extend(character.to_uppercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    normalized
}

fn score_unicode_name(name: &str, terms: &[&str]) -> Option<u32> {
    let all_whole = terms.iter().all(|term| name_has_term(name, term, true));
    let compact_query_len = terms.iter().map(|term| term.len()).sum::<usize>() as u32;
    let length_bonus = 10_000u32.saturating_sub(name.len() as u32);
    let score = if all_whole {
        2_000_000
    } else if terms.iter().all(|term| name_has_term(name, term, false)) {
        1_500_000
    } else if terms.iter().all(|term| name.contains(term)) {
        1_000_000
    } else {
        return None;
    };
    Some(score + compact_query_len * 100 + length_bonus)
}

fn score_cldr_annotation(annotation: Annotation<'_>, query: &str, terms: &[&str]) -> Option<u32> {
    if annotation.short_name.eq_ignore_ascii_case(query) {
        return Some(3_750_000);
    }
    score_unicode_name(annotation.searchable, terms).map(|score| score + 250_000)
}

fn name_has_term(name: &str, term: &str, require_right_boundary: bool) -> bool {
    name.match_indices(term).any(|(start, _)| {
        let left_boundary = name[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_alphanumeric());
        let end = start + term.len();
        let right_boundary = name[end..]
            .chars()
            .next()
            .is_none_or(|character| !character.is_alphanumeric());
        left_boundary && (!require_right_boundary || right_boundary)
    })
}

fn score_curated_character(entry: &CuratedCharacter, query: &str, terms: &[&str]) -> Option<u32> {
    if entry.text == query || entry.name.eq_ignore_ascii_case(query) {
        return Some(4_500_000);
    }
    let searchable = format!(
        "{} {} {} {} {}",
        entry.name,
        entry.category,
        entry.aliases,
        display_codepoints(entry.text),
        entry.text
    )
    .to_ascii_uppercase();
    if terms.is_empty() {
        return searchable.contains(query).then_some(3_000_000);
    }
    terms
        .iter()
        .all(|term| searchable.contains(term))
        .then_some(2_500_000 + terms.iter().map(|term| term.len()).sum::<usize>() as u32 * 100)
}

fn named_result(character: char, name: &str, score: u32) -> RankedCharacter {
    let text = character.to_string();
    let id = stable_character_id("named", &text);
    RankedCharacter {
        score,
        tie_break: format!("{:06X}", character as u32),
        selection: UnicodePickerSelection {
            id: id.clone(),
            text: text.clone(),
        },
        command: RegisteredCommand {
            id,
            title: format!("{text}  {name}"),
            category: format!(
                "Unicode {UNICODE_DATA_VERSION} · {}",
                display_codepoints(&text)
            ),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
    }
}

fn cldr_result(annotation: Annotation<'_>, score: u32) -> RankedCharacter {
    let id = stable_character_id("cldr", annotation.text);
    RankedCharacter {
        score,
        tie_break: format!("cldr-{}", display_codepoints(annotation.text)),
        selection: UnicodePickerSelection {
            id: id.clone(),
            text: annotation.text.into(),
        },
        command: RegisteredCommand {
            id,
            title: format!("{}  {}", annotation.text, annotation.short_name),
            category: format!(
                "CLDR {CLDR_VERSION} {CLDR_LOCALE} · {}",
                display_codepoints(annotation.text)
            ),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
    }
}

fn ranked_results_owned_bytes(results: &Vec<RankedCharacter>) -> usize {
    results.capacity() * size_of::<RankedCharacter>()
        + results
            .iter()
            .map(|result| {
                result.tie_break.capacity()
                    + result.selection.id.capacity()
                    + result.selection.text.capacity()
                    + result.command.id.capacity()
                    + result.command.title.capacity()
                    + result.command.category.capacity()
                    + result.command.shortcut.as_ref().map_or(0, String::capacity)
                    + match &result.command.source {
                        CommandSource::Plugin(plugin) => plugin.capacity(),
                        CommandSource::Core | CommandSource::LaunchProfiles => 0,
                    }
            })
            .sum::<usize>()
}

#[cfg(test)]
pub(crate) struct UnicodePickerBenchmark {
    pub annotation_validation_ms: Vec<f64>,
    pub keyword_query_ms: Vec<f64>,
    pub annotation_static_bytes: Vec<f64>,
    pub result_owned_bytes: Vec<f64>,
}

#[cfg(test)]
pub(crate) fn benchmark_unicode_picker() -> UnicodePickerBenchmark {
    let mut annotation_validation_ms = Vec::with_capacity(5);
    let mut annotation_static_bytes = Vec::with_capacity(5);
    for _ in 0..5 {
        let started = std::time::Instant::now();
        let index = annotations::validate_embedded_index();
        std::hint::black_box(index.annotation(index.len() - 1));
        annotation_validation_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
        annotation_static_bytes.push(index.static_bytes() as f64);
    }

    unicode_name_index();
    annotations::index();
    let mut keyword_query_ms = Vec::with_capacity(5);
    let mut result_owned_bytes = Vec::with_capacity(5);
    for (query, expected) in [
        ("heart blue", "💙"),
        ("party pinata", "🪅"),
        ("thumb medium skin tone", "👍🏽"),
        ("flag united states", "🇺🇸"),
        ("woman judge medium dark skin tone", "👩🏾‍⚖️"),
    ] {
        let started = std::time::Instant::now();
        let results = search_unicode_characters(query);
        keyword_query_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
        assert!(
            results
                .iter()
                .any(|result| result.selection.text == expected),
            "benchmark query {query:?} missed {expected:?}"
        );
        result_owned_bytes.push(ranked_results_owned_bytes(&results) as f64);
        std::hint::black_box(results);
    }

    UnicodePickerBenchmark {
        annotation_validation_ms,
        keyword_query_ms,
        annotation_static_bytes,
        result_owned_bytes,
    }
}

fn curated_result(entry: &CuratedCharacter, score: u32, position: usize) -> RankedCharacter {
    let id = stable_character_id("curated", entry.text);
    RankedCharacter {
        score,
        tie_break: format!("curated-{position:03}"),
        selection: UnicodePickerSelection {
            id: id.clone(),
            text: entry.text.into(),
        },
        command: RegisteredCommand {
            id,
            title: format!("{}  {}", entry.text, entry.name),
            category: format!(
                "{} · {} · {}",
                entry.category,
                display_codepoints(entry.text),
                entry.aliases
            ),
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        },
    }
}

impl Default for UnicodePicker {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_scalar_query(value: &str) -> Option<char> {
    let value = value.trim();
    let digits = value
        .strip_prefix("U+")
        .or_else(|| value.strip_prefix("u+"))
        .or_else(|| value.strip_prefix("0x"))
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if digits.is_empty() || digits.len() > 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let character = char::from_u32(u32::from_str_radix(digits, 16).ok()?)?;
    (!character.is_control()).then_some(character)
}

fn looks_like_scalar_query(value: &str) -> bool {
    let value = value.trim();
    if value.starts_with("U+")
        || value.starts_with("u+")
        || value.starts_with("0x")
        || value.starts_with("0X")
    {
        return true;
    }
    (4..=6).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn stable_character_id(kind: &str, text: &str) -> String {
    let codepoints = text
        .chars()
        .map(|character| format!("{:x}", character as u32))
        .collect::<Vec<_>>()
        .join("-");
    format!("unicode-picker.{kind}.{codepoints}")
}

fn display_codepoints(text: &str) -> String {
    text.chars()
        .map(|character| format!("U+{:04X}", character as u32))
        .collect::<Vec<_>>()
        .join(" ")
}

const CURATED_CHARACTERS: &[CuratedCharacter] = &[
    CuratedCharacter {
        text: "–",
        name: "EN DASH",
        category: "Punctuation",
        aliases: "dash range",
    },
    CuratedCharacter {
        text: "—",
        name: "EM DASH",
        category: "Punctuation",
        aliases: "dash break",
    },
    CuratedCharacter {
        text: "…",
        name: "HORIZONTAL ELLIPSIS",
        category: "Punctuation",
        aliases: "dots omission",
    },
    CuratedCharacter {
        text: "‘",
        name: "LEFT SINGLE QUOTATION MARK",
        category: "Punctuation",
        aliases: "smart quote apostrophe",
    },
    CuratedCharacter {
        text: "’",
        name: "RIGHT SINGLE QUOTATION MARK",
        category: "Punctuation",
        aliases: "smart quote apostrophe",
    },
    CuratedCharacter {
        text: "“",
        name: "LEFT DOUBLE QUOTATION MARK",
        category: "Punctuation",
        aliases: "smart quote",
    },
    CuratedCharacter {
        text: "”",
        name: "RIGHT DOUBLE QUOTATION MARK",
        category: "Punctuation",
        aliases: "smart quote",
    },
    CuratedCharacter {
        text: "•",
        name: "BULLET",
        category: "Punctuation",
        aliases: "list dot",
    },
    CuratedCharacter {
        text: "·",
        name: "MIDDLE DOT",
        category: "Punctuation",
        aliases: "interpunct separator",
    },
    CuratedCharacter {
        text: "§",
        name: "SECTION SIGN",
        category: "Punctuation",
        aliases: "legal section",
    },
    CuratedCharacter {
        text: "¶",
        name: "PILCROW SIGN",
        category: "Punctuation",
        aliases: "paragraph",
    },
    CuratedCharacter {
        text: "©",
        name: "COPYRIGHT SIGN",
        category: "Punctuation",
        aliases: "copyright",
    },
    CuratedCharacter {
        text: "®",
        name: "REGISTERED SIGN",
        category: "Punctuation",
        aliases: "registered trademark",
    },
    CuratedCharacter {
        text: "™",
        name: "TRADE MARK SIGN",
        category: "Punctuation",
        aliases: "trademark",
    },
    CuratedCharacter {
        text: "€",
        name: "EURO SIGN",
        category: "Currency",
        aliases: "money eur",
    },
    CuratedCharacter {
        text: "£",
        name: "POUND SIGN",
        category: "Currency",
        aliases: "money sterling gbp",
    },
    CuratedCharacter {
        text: "¥",
        name: "YEN SIGN",
        category: "Currency",
        aliases: "money yuan jpy cny",
    },
    CuratedCharacter {
        text: "₹",
        name: "INDIAN RUPEE SIGN",
        category: "Currency",
        aliases: "money inr",
    },
    CuratedCharacter {
        text: "₩",
        name: "WON SIGN",
        category: "Currency",
        aliases: "money krw",
    },
    CuratedCharacter {
        text: "₽",
        name: "RUBLE SIGN",
        category: "Currency",
        aliases: "money rub",
    },
    CuratedCharacter {
        text: "₿",
        name: "BITCOIN SIGN",
        category: "Currency",
        aliases: "money btc crypto",
    },
    CuratedCharacter {
        text: "¢",
        name: "CENT SIGN",
        category: "Currency",
        aliases: "money cents",
    },
    CuratedCharacter {
        text: "←",
        name: "LEFTWARDS ARROW",
        category: "Arrows",
        aliases: "left back previous",
    },
    CuratedCharacter {
        text: "→",
        name: "RIGHTWARDS ARROW",
        category: "Arrows",
        aliases: "right forward next",
    },
    CuratedCharacter {
        text: "↑",
        name: "UPWARDS ARROW",
        category: "Arrows",
        aliases: "up",
    },
    CuratedCharacter {
        text: "↓",
        name: "DOWNWARDS ARROW",
        category: "Arrows",
        aliases: "down",
    },
    CuratedCharacter {
        text: "↔",
        name: "LEFT RIGHT ARROW",
        category: "Arrows",
        aliases: "horizontal bidirectional",
    },
    CuratedCharacter {
        text: "↕",
        name: "UP DOWN ARROW",
        category: "Arrows",
        aliases: "vertical bidirectional",
    },
    CuratedCharacter {
        text: "↖",
        name: "NORTH WEST ARROW",
        category: "Arrows",
        aliases: "upper left diagonal",
    },
    CuratedCharacter {
        text: "↗",
        name: "NORTH EAST ARROW",
        category: "Arrows",
        aliases: "upper right diagonal",
    },
    CuratedCharacter {
        text: "↘",
        name: "SOUTH EAST ARROW",
        category: "Arrows",
        aliases: "lower right diagonal",
    },
    CuratedCharacter {
        text: "↙",
        name: "SOUTH WEST ARROW",
        category: "Arrows",
        aliases: "lower left diagonal",
    },
    CuratedCharacter {
        text: "⇒",
        name: "RIGHTWARDS DOUBLE ARROW",
        category: "Arrows",
        aliases: "implies fat arrow",
    },
    CuratedCharacter {
        text: "⇐",
        name: "LEFTWARDS DOUBLE ARROW",
        category: "Arrows",
        aliases: "fat arrow",
    },
    CuratedCharacter {
        text: "⇔",
        name: "LEFT RIGHT DOUBLE ARROW",
        category: "Arrows",
        aliases: "equivalent iff",
    },
    CuratedCharacter {
        text: "↩",
        name: "LEFTWARDS ARROW WITH HOOK",
        category: "Arrows",
        aliases: "return undo",
    },
    CuratedCharacter {
        text: "↪",
        name: "RIGHTWARDS ARROW WITH HOOK",
        category: "Arrows",
        aliases: "redirect redo",
    },
    CuratedCharacter {
        text: "↻",
        name: "CLOCKWISE OPEN CIRCLE ARROW",
        category: "Arrows",
        aliases: "reload refresh rotate",
    },
    CuratedCharacter {
        text: "±",
        name: "PLUS-MINUS SIGN",
        category: "Mathematics",
        aliases: "plus minus",
    },
    CuratedCharacter {
        text: "×",
        name: "MULTIPLICATION SIGN",
        category: "Mathematics",
        aliases: "times multiply cross",
    },
    CuratedCharacter {
        text: "÷",
        name: "DIVISION SIGN",
        category: "Mathematics",
        aliases: "divide obelus",
    },
    CuratedCharacter {
        text: "≠",
        name: "NOT EQUAL TO",
        category: "Mathematics",
        aliases: "inequality",
    },
    CuratedCharacter {
        text: "≈",
        name: "ALMOST EQUAL TO",
        category: "Mathematics",
        aliases: "approximately",
    },
    CuratedCharacter {
        text: "≤",
        name: "LESS-THAN OR EQUAL TO",
        category: "Mathematics",
        aliases: "lte",
    },
    CuratedCharacter {
        text: "≥",
        name: "GREATER-THAN OR EQUAL TO",
        category: "Mathematics",
        aliases: "gte",
    },
    CuratedCharacter {
        text: "∞",
        name: "INFINITY",
        category: "Mathematics",
        aliases: "infinite",
    },
    CuratedCharacter {
        text: "√",
        name: "SQUARE ROOT",
        category: "Mathematics",
        aliases: "radical",
    },
    CuratedCharacter {
        text: "∑",
        name: "N-ARY SUMMATION",
        category: "Mathematics",
        aliases: "sum sigma",
    },
    CuratedCharacter {
        text: "∏",
        name: "N-ARY PRODUCT",
        category: "Mathematics",
        aliases: "product pi",
    },
    CuratedCharacter {
        text: "∫",
        name: "INTEGRAL",
        category: "Mathematics",
        aliases: "calculus",
    },
    CuratedCharacter {
        text: "∂",
        name: "PARTIAL DIFFERENTIAL",
        category: "Mathematics",
        aliases: "derivative",
    },
    CuratedCharacter {
        text: "∇",
        name: "NABLA",
        category: "Mathematics",
        aliases: "gradient del",
    },
    CuratedCharacter {
        text: "∈",
        name: "ELEMENT OF",
        category: "Mathematics",
        aliases: "set member",
    },
    CuratedCharacter {
        text: "∉",
        name: "NOT AN ELEMENT OF",
        category: "Mathematics",
        aliases: "set not member",
    },
    CuratedCharacter {
        text: "∅",
        name: "EMPTY SET",
        category: "Mathematics",
        aliases: "null set",
    },
    CuratedCharacter {
        text: "∩",
        name: "INTERSECTION",
        category: "Mathematics",
        aliases: "set and",
    },
    CuratedCharacter {
        text: "∪",
        name: "UNION",
        category: "Mathematics",
        aliases: "set or",
    },
    CuratedCharacter {
        text: "⌘",
        name: "PLACE OF INTEREST SIGN",
        category: "Technical",
        aliases: "command cmd apple",
    },
    CuratedCharacter {
        text: "⌥",
        name: "OPTION KEY",
        category: "Technical",
        aliases: "option alt apple",
    },
    CuratedCharacter {
        text: "⌃",
        name: "UP ARROWHEAD",
        category: "Technical",
        aliases: "control ctrl key",
    },
    CuratedCharacter {
        text: "⇧",
        name: "UPWARDS WHITE ARROW",
        category: "Technical",
        aliases: "shift key",
    },
    CuratedCharacter {
        text: "⌫",
        name: "ERASE TO THE LEFT",
        category: "Technical",
        aliases: "backspace delete key",
    },
    CuratedCharacter {
        text: "⏎",
        name: "RETURN SYMBOL",
        category: "Technical",
        aliases: "enter return key",
    },
    CuratedCharacter {
        text: "⎋",
        name: "BROKEN CIRCLE WITH NORTHWEST ARROW",
        category: "Technical",
        aliases: "escape esc key",
    },
    CuratedCharacter {
        text: "⏻",
        name: "POWER SYMBOL",
        category: "Technical",
        aliases: "power on off",
    },
    CuratedCharacter {
        text: "⚙",
        name: "GEAR",
        category: "Technical",
        aliases: "settings configuration",
    },
    CuratedCharacter {
        text: "─",
        name: "BOX DRAWINGS LIGHT HORIZONTAL",
        category: "Box Drawing",
        aliases: "line border",
    },
    CuratedCharacter {
        text: "│",
        name: "BOX DRAWINGS LIGHT VERTICAL",
        category: "Box Drawing",
        aliases: "line border",
    },
    CuratedCharacter {
        text: "┌",
        name: "BOX DRAWINGS LIGHT DOWN AND RIGHT",
        category: "Box Drawing",
        aliases: "corner top left",
    },
    CuratedCharacter {
        text: "┐",
        name: "BOX DRAWINGS LIGHT DOWN AND LEFT",
        category: "Box Drawing",
        aliases: "corner top right",
    },
    CuratedCharacter {
        text: "└",
        name: "BOX DRAWINGS LIGHT UP AND RIGHT",
        category: "Box Drawing",
        aliases: "corner bottom left",
    },
    CuratedCharacter {
        text: "┘",
        name: "BOX DRAWINGS LIGHT UP AND LEFT",
        category: "Box Drawing",
        aliases: "corner bottom right",
    },
    CuratedCharacter {
        text: "├",
        name: "BOX DRAWINGS LIGHT VERTICAL AND RIGHT",
        category: "Box Drawing",
        aliases: "tee branch",
    },
    CuratedCharacter {
        text: "┤",
        name: "BOX DRAWINGS LIGHT VERTICAL AND LEFT",
        category: "Box Drawing",
        aliases: "tee branch",
    },
    CuratedCharacter {
        text: "┬",
        name: "BOX DRAWINGS LIGHT DOWN AND HORIZONTAL",
        category: "Box Drawing",
        aliases: "tee top",
    },
    CuratedCharacter {
        text: "┴",
        name: "BOX DRAWINGS LIGHT UP AND HORIZONTAL",
        category: "Box Drawing",
        aliases: "tee bottom",
    },
    CuratedCharacter {
        text: "┼",
        name: "BOX DRAWINGS LIGHT VERTICAL AND HORIZONTAL",
        category: "Box Drawing",
        aliases: "cross junction",
    },
    CuratedCharacter {
        text: "═",
        name: "BOX DRAWINGS DOUBLE HORIZONTAL",
        category: "Box Drawing",
        aliases: "line border",
    },
    CuratedCharacter {
        text: "║",
        name: "BOX DRAWINGS DOUBLE VERTICAL",
        category: "Box Drawing",
        aliases: "line border",
    },
    CuratedCharacter {
        text: "╔",
        name: "BOX DRAWINGS DOUBLE DOWN AND RIGHT",
        category: "Box Drawing",
        aliases: "corner top left",
    },
    CuratedCharacter {
        text: "╗",
        name: "BOX DRAWINGS DOUBLE DOWN AND LEFT",
        category: "Box Drawing",
        aliases: "corner top right",
    },
    CuratedCharacter {
        text: "╚",
        name: "BOX DRAWINGS DOUBLE UP AND RIGHT",
        category: "Box Drawing",
        aliases: "corner bottom left",
    },
    CuratedCharacter {
        text: "╝",
        name: "BOX DRAWINGS DOUBLE UP AND LEFT",
        category: "Box Drawing",
        aliases: "corner bottom right",
    },
    CuratedCharacter {
        text: "╬",
        name: "BOX DRAWINGS DOUBLE VERTICAL AND HORIZONTAL",
        category: "Box Drawing",
        aliases: "cross junction",
    },
    CuratedCharacter {
        text: "●",
        name: "BLACK CIRCLE",
        category: "Shapes",
        aliases: "filled dot round",
    },
    CuratedCharacter {
        text: "○",
        name: "WHITE CIRCLE",
        category: "Shapes",
        aliases: "outline dot round",
    },
    CuratedCharacter {
        text: "■",
        name: "BLACK SQUARE",
        category: "Shapes",
        aliases: "filled box",
    },
    CuratedCharacter {
        text: "□",
        name: "WHITE SQUARE",
        category: "Shapes",
        aliases: "outline box",
    },
    CuratedCharacter {
        text: "▲",
        name: "BLACK UP-POINTING TRIANGLE",
        category: "Shapes",
        aliases: "filled up",
    },
    CuratedCharacter {
        text: "△",
        name: "WHITE UP-POINTING TRIANGLE",
        category: "Shapes",
        aliases: "outline up",
    },
    CuratedCharacter {
        text: "◆",
        name: "BLACK DIAMOND",
        category: "Shapes",
        aliases: "filled rhombus",
    },
    CuratedCharacter {
        text: "◇",
        name: "WHITE DIAMOND",
        category: "Shapes",
        aliases: "outline rhombus",
    },
    CuratedCharacter {
        text: "★",
        name: "BLACK STAR",
        category: "Shapes",
        aliases: "filled favorite",
    },
    CuratedCharacter {
        text: "☆",
        name: "WHITE STAR",
        category: "Shapes",
        aliases: "outline favorite",
    },
    CuratedCharacter {
        text: "✓",
        name: "CHECK MARK",
        category: "Shapes",
        aliases: "tick success yes",
    },
    CuratedCharacter {
        text: "✕",
        name: "MULTIPLICATION X",
        category: "Shapes",
        aliases: "cross close no",
    },
    CuratedCharacter {
        text: "♥",
        name: "BLACK HEART SUIT",
        category: "Shapes",
        aliases: "heart love",
    },
    CuratedCharacter {
        text: "😀",
        name: "GRINNING FACE",
        category: "Emoji",
        aliases: "smile happy",
    },
    CuratedCharacter {
        text: "😂",
        name: "FACE WITH TEARS OF JOY",
        category: "Emoji",
        aliases: "laugh cry lol",
    },
    CuratedCharacter {
        text: "😊",
        name: "SMILING FACE WITH SMILING EYES",
        category: "Emoji",
        aliases: "happy blush",
    },
    CuratedCharacter {
        text: "😍",
        name: "SMILING FACE WITH HEART-EYES",
        category: "Emoji",
        aliases: "love heart eyes",
    },
    CuratedCharacter {
        text: "🤔",
        name: "THINKING FACE",
        category: "Emoji",
        aliases: "hmm consider",
    },
    CuratedCharacter {
        text: "👍",
        name: "THUMBS UP SIGN",
        category: "Emoji",
        aliases: "like approve yes",
    },
    CuratedCharacter {
        text: "👎",
        name: "THUMBS DOWN SIGN",
        category: "Emoji",
        aliases: "dislike reject no",
    },
    CuratedCharacter {
        text: "🎉",
        name: "PARTY POPPER",
        category: "Emoji",
        aliases: "celebrate congratulations",
    },
    CuratedCharacter {
        text: "🔥",
        name: "FIRE",
        category: "Emoji",
        aliases: "hot flame",
    },
    CuratedCharacter {
        text: "🚀",
        name: "ROCKET",
        category: "Emoji",
        aliases: "launch ship",
    },
    CuratedCharacter {
        text: "💡",
        name: "ELECTRIC LIGHT BULB",
        category: "Emoji",
        aliases: "idea light",
    },
    CuratedCharacter {
        text: "⚠️",
        name: "WARNING SIGN",
        category: "Emoji",
        aliases: "caution alert",
    },
    CuratedCharacter {
        text: "✅",
        name: "WHITE HEAVY CHECK MARK",
        category: "Emoji",
        aliases: "success done yes",
    },
    CuratedCharacter {
        text: "❌",
        name: "CROSS MARK",
        category: "Emoji",
        aliases: "error failure no",
    },
    CuratedCharacter {
        text: "❤️",
        name: "HEAVY BLACK HEART",
        category: "Emoji",
        aliases: "love red heart",
    },
    CuratedCharacter {
        text: "🧑‍💻",
        name: "TECHNOLOGIST",
        category: "Emoji",
        aliases: "developer coder programmer",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn filter(picker: &mut UnicodePicker, value: &str) -> PaletteInfo {
        assert!(picker.accessibility_set_filter(value));
        picker.build_info()
    }

    #[test]
    fn curated_index_is_bounded_searchable_and_category_complete() {
        let mut picker = UnicodePicker::new();
        picker.open();
        let info = picker.build_info();
        assert!(info.total_count >= 100);
        assert!(info.total_count <= 256);
        for query in [
            "punctuation",
            "currency",
            "arrows",
            "mathematics",
            "technical",
            "box drawing",
            "shapes",
            "emoji",
        ] {
            assert!(filter(&mut picker, query).total_entries > 0, "{query}");
        }
        assert!(filter(&mut picker, "reload").entries[0].title.contains('↻'));
        assert!(filter(&mut picker, "U+2192").entries[0].title.contains('→'));
    }

    #[test]
    fn canonical_names_cover_non_curated_and_algorithmic_characters() {
        let mut picker = UnicodePicker::new();
        picker.open();

        let info = filter(&mut picker, "latin letter ezh");
        assert!(info.entries.iter().any(|entry| entry.title.contains('ʒ')));

        let info = filter(&mut picker, "hangul syllable gag");
        assert_eq!(info.entries[0].id, "unicode-picker.named.ac01");
        assert!(info.entries[0].title.contains('각'));

        let info = filter(&mut picker, "cjk unified ideograph 4e00");
        assert_eq!(info.entries[0].id, "unicode-picker.named.4e00");
        assert!(info.entries[0].title.contains('一'));
    }

    #[test]
    fn exact_aliases_use_loose_matching_but_never_expose_controls() {
        let mut picker = UnicodePicker::new();
        picker.open();
        let info = filter(&mut picker, "byte_order-mark");
        assert_eq!(info.entries[0].id, "unicode-picker.named.feff");
        assert_eq!(picker.activate().unwrap().text, "\u{feff}");

        let mut picker = UnicodePicker::new();
        picker.open();
        filter(&mut picker, "new line");
        assert!(picker.actions.iter().all(|selection| {
            !selection.text.is_empty() && !selection.text.chars().any(char::is_control)
        }));
    }

    #[test]
    fn exhaustive_search_has_explicit_result_memory_and_latency_bounds() {
        let index = unicode_name_index();
        assert_eq!(index.records.len(), 153_626);
        assert!(index.retained_bytes() <= UNICODE_SEARCH_MEMORY_BUDGET);

        let started = Instant::now();
        for query in [
            "letter",
            "black",
            "arabic letter",
            "mathematical symbol",
            "cjk unified",
        ] {
            let query_started = Instant::now();
            let results = search_unicode_characters(query);
            assert!(!results.is_empty(), "{query}");
            assert!(results.len() <= UNICODE_SEARCH_RESULT_LIMIT, "{query}");
            if query == "letter" {
                assert_eq!(results.len(), UNICODE_SEARCH_RESULT_LIMIT);
            }
            eprintln!("unicode-query {query:?} took {:?}", query_started.elapsed());
        }
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "five warm bounded queries took {:?}",
            started.elapsed()
        );
        eprintln!(
            "unicode-index records={} retained_bytes={} five_queries={:?}",
            index.records.len(),
            index.retained_bytes(),
            started.elapsed()
        );
    }

    #[test]
    fn cldr_keywords_find_exact_single_and_multi_scalar_emoji() {
        for (query, expected) in [
            ("good thumb yes", "👍"),
            ("thumb medium skin tone", "👍🏽"),
            ("flag united states", "🇺🇸"),
            ("woman judge medium dark skin tone", "👩🏾‍⚖️"),
            ("transgender flag", "🏳️‍⚧️"),
            ("party pinata", "🪅"),
        ] {
            let mut picker = UnicodePicker::new();
            picker.open();
            let info = filter(&mut picker, query);
            let entry = info
                .entries
                .iter()
                .find(|entry| entry.title.starts_with(expected))
                .unwrap_or_else(|| panic!("{query:?} did not find {expected:?}"));
            let selection = picker.execute_command(&entry.id).unwrap();
            assert_eq!(selection.text.as_bytes(), expected.as_bytes(), "{query}");
            assert_eq!(selection.id, stable_character_id("cldr", expected));
        }
    }

    #[test]
    fn canonical_curated_and_exact_scalar_precedence_remains_stable() {
        let mut picker = UnicodePicker::new();
        picker.open();
        assert_eq!(
            filter(&mut picker, "snowman").entries[0].id,
            "unicode-picker.named.2603"
        );
        assert_eq!(
            filter(&mut picker, "technologist").entries[0].id,
            "unicode-picker.curated.1f9d1-200d-1f4bb"
        );
        assert_eq!(
            filter(&mut picker, "U+2764").entries[0].id,
            "unicode-picker.codepoint.2764"
        );
        assert_eq!(picker.activate().unwrap().text, "❤");
    }

    #[test]
    fn cldr_search_state_is_deduplicated_and_memory_bounded() {
        let annotations = annotations::index();
        assert_eq!(annotations.len(), annotations::CLDR_RECORD_COUNT);
        assert_eq!(annotations.static_bytes(), 475_510);
        assert!(annotations.static_bytes() <= annotations::CLDR_STATIC_BYTES_BUDGET);

        for query in ["heart", "flag", "woman", "party", "piñata"] {
            let results = search_unicode_characters(query);
            assert!(!results.is_empty(), "{query}");
            assert!(results.len() <= UNICODE_SEARCH_RESULT_LIMIT, "{query}");
            assert!(
                ranked_results_owned_bytes(&results) <= UNICODE_RESULT_MEMORY_BUDGET,
                "{query}"
            );
            let unique = results
                .iter()
                .map(|result| result.selection.text.as_str())
                .collect::<HashSet<_>>();
            assert_eq!(unique.len(), results.len(), "{query}");
        }
    }

    #[test]
    fn unicode_letters_do_not_create_false_token_boundaries() {
        let score = score_unicode_name("PIÑATA PARTY", &["ATA"]).unwrap();
        assert!(
            score < 1_500_000,
            "substring was promoted to a token: {score}"
        );
        assert!(score_unicode_name("PIÑATA PARTY", &["PIÑATA"]).unwrap() >= 2_000_000);
    }

    #[test]
    fn empty_query_restores_only_the_curated_catalog() {
        let mut picker = UnicodePicker::new();
        picker.open();
        assert!(
            filter(&mut picker, "snowman")
                .entries
                .iter()
                .any(|entry| entry.id == "unicode-picker.named.2603")
        );
        let info = filter(&mut picker, "");
        assert_eq!(info.total_count, CURATED_CHARACTERS.len());
        assert!(info.total_count <= UNICODE_SEARCH_RESULT_LIMIT);
    }

    #[test]
    fn exact_codepoint_syntaxes_accept_scalars_and_reject_unsafe_values() {
        for query in ["U+1F9E1", "u+1f9e1", "0x1F9E1", "1f9e1", "BEEF"] {
            let mut picker = UnicodePicker::new();
            picker.open();
            let info = filter(&mut picker, query);
            assert_eq!(info.total_entries, 1, "{query}");
            let expected = if query == "BEEF" { "뻯" } else { "🧡" };
            assert_eq!(picker.activate().unwrap().text, expected);
        }
        for query in [
            "U+0000",
            "U+001B",
            "U+007F",
            "U+0085",
            "U+D800",
            "U+110000",
            "U+XYZ",
            "U+1234567",
        ] {
            let mut picker = UnicodePicker::new();
            picker.open();
            assert_eq!(filter(&mut picker, query).total_entries, 0, "{query}");
            assert!(picker.activate().is_none());
        }
    }

    #[test]
    fn curated_graphemes_and_stable_ids_preserve_exact_bytes() {
        let mut picker = UnicodePicker::new();
        picker.open();
        let info = filter(&mut picker, "technologist");
        assert!(info.total_entries >= 1);
        let id = info.entries[0].id.clone();
        assert_eq!(id, "unicode-picker.curated.1f9d1-200d-1f4bb");
        assert_eq!(
            picker.execute_command(&id).unwrap().text.as_bytes(),
            "🧑‍💻".as_bytes()
        );
        assert!(!picker.is_active());
    }

    #[test]
    fn stale_or_filtered_ids_cannot_execute() {
        let mut picker = UnicodePicker::new();
        picker.open();
        let stale = filter(&mut picker, "technologist").entries[0].id.clone();
        filter(&mut picker, "U+20AC");
        assert!(picker.execute_command(&stale).is_none());
        assert!(picker.is_active());
        assert!(filter(&mut picker, "euro").entries[0].title.contains('€'));
    }

    #[test]
    fn shared_palette_geometry_executes_the_clicked_character() {
        let mut picker = UnicodePicker::new();
        picker.open();
        let info = filter(&mut picker, "euro");
        let layout = PaletteLayout::calculate(900.0, 700.0, 10.0, 20.0, info.animation, 1);
        let selection = picker
            .handle_click(
                layout.panel_x + layout.padding,
                layout.list_y + layout.row_height * 0.5,
                &layout,
            )
            .unwrap();
        assert_eq!(selection.text, "€");

        let mut picker = UnicodePicker::new();
        picker.open();
        let info = filter(&mut picker, "snowman");
        let layout = PaletteLayout::calculate(900.0, 700.0, 10.0, 20.0, info.animation, 1);
        let selection = picker
            .handle_click(
                layout.panel_x + layout.padding,
                layout.list_y + layout.row_height * 0.5,
                &layout,
            )
            .unwrap();
        assert_eq!(selection.id, "unicode-picker.named.2603");
        assert_eq!(selection.text, "☃");
    }

    #[test]
    fn reduced_motion_opens_without_scheduling_transition_frames() {
        let mut picker = UnicodePicker::new();
        picker.set_reduced_motion(true);
        picker.open();
        assert_eq!(picker.build_info().animation, 1.0);
        assert!(!picker.update_animation());
    }
}
