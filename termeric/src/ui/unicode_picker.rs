//! Bounded Unicode character selection built on the shared palette component.

use crate::ui::command_palette::palette::{CommandSource, RegisteredCommand};
use crate::ui::command_palette::{CommandPalette, PaletteInfo, PaletteLayout, PaletteResult};

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
    exact_mode: bool,
}

impl UnicodePicker {
    pub fn new() -> Self {
        let mut palette = CommandPalette::new();
        palette.set_presentation("Search characters or enter U+2192...", "character", "Close");
        Self {
            palette,
            actions: Vec::new(),
            exact_mode: false,
        }
    }

    pub fn open(&mut self) {
        self.rebuild_registry();
        self.palette.open();
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
                self.exact_mode = true;
                self.actions.clear();
                self.palette.replace_all_commands(Vec::new());
            }
        } else if self.exact_mode {
            self.rebuild_registry();
        }
    }

    fn rebuild_registry(&mut self) {
        self.exact_mode = false;
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
        self.exact_mode = true;
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
        assert_eq!(info.total_entries, 1);
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
