//! Writing `config.toml`. A save changes only the settings whose values
//! changed since the file was loaded or last saved, so the person's comments,
//! layout, and every other key stay as they wrote them, including a key
//! edited in another program meanwhile. A reset keeps the old file beside
//! the new one.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow, bail};
use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

use super::Config;

/// What a reset leaves in the file: nothing is set, so every setting is at
/// its default.
pub const RESET_TEXT: &str = "\
# Termeric configuration. Every setting is at its default.
# Add only the settings you want to change. config.example.toml, in Termeric's
# documentation directory (such as /usr/share/doc/termeric), lists them all.
";

/// How many names a reset tries for its copy of the old file within one
/// second before giving up.
const MAX_BACKUP_NAMES: u32 = 100;

/// `config` as the table a save compares: every setting, as the file would
/// hold it if each one were written out.
pub fn config_table(config: &Config) -> Result<toml::Table> {
    match toml::Value::try_from(config).context("serialize the configuration")? {
        toml::Value::Table(table) => Ok(table),
        other => bail!(
            "the configuration serialized as {} rather than a table",
            other.type_str()
        ),
    }
}

/// Bring the file at `path` from `baseline`, the configuration it held when
/// it was loaded or last saved (from [`config_table`]), to `live`, changing
/// only the settings that differ between the two. A file that does not load
/// is left as it is: Termeric is then running on its defaults or on the last
/// settings that did load, and the person's own file may be one typo away
/// from working. A symbolic link is followed and its file replaced; a save
/// with nothing to change writes nothing.
///
/// Returns the configuration the file now holds, as [`config_table`] gives
/// it. That is more than `live` when another program changed the file since
/// it was loaded.
pub fn save(path: &Path, baseline: &toml::Table, live: &Config) -> Result<toml::Table> {
    let target = link_target(path)?;
    let original = read_if_present(&target)?.unwrap_or_default();
    let (mut document, current) = loadable_document(path, &original)?;
    let current = config_table(&current)?;
    let live_table = config_table(live)?;
    // What is added is written as a full save would write it, in the
    // settings' own order, with tables under headers.
    let written: DocumentMut = toml::to_string_pretty(live)
        .context("serialize the configuration")?
        .parse()
        .context("parse the serialized configuration")?;
    apply_document_changes(
        document.as_table_mut(),
        false,
        written.as_table(),
        &current,
        baseline,
        &live_table,
    );
    let updated = document.to_string();

    // The edited document must load as exactly the settings that the same
    // changes make to the file's values; anything else is refused.
    let refused = || {
        format!(
            "{} could not be changed without also changing other settings, so it was left as it is",
            path.display()
        )
    };
    let file = config_table(&Config::from_toml(&updated, path).with_context(refused)?)?;
    let mut expected: toml::Table =
        toml::from_str(&original).with_context(|| format!("failed to parse {}", path.display()))?;
    apply_value_changes(&mut expected, baseline, &live_table);
    let expected: Config = toml::Value::Table(expected)
        .try_into()
        .with_context(refused)?;
    if config_table(&expected)? != file {
        bail!(refused());
    }

    if updated != original {
        replace_file(&target, &updated)?;
    }
    Ok(file)
}

/// Replace the file at `path` with [`RESET_TEXT`], first copying the old
/// file, whether or not it loads, beside `path` as
/// `config.toml.<seconds>.bak` (from `now`) without replacing an earlier
/// copy. Returns the copy's path, or `None` when there was no file. A
/// symbolic link is followed and its file replaced.
///
/// Reset is the way out of a configuration that cannot load, so the copy is
/// streamed and has no size limit: a file too large to load still resets.
pub fn reset(path: &Path, now: SystemTime) -> Result<Option<PathBuf>> {
    let target = link_target(path)?;
    let backup = match crate::safe_fs::open_regular_any_size(&target) {
        Ok(old) => Some(keep_copy(path, old, now)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("read {}", target.display())),
    };
    replace_file(&target, RESET_TEXT)?;
    Ok(backup)
}

/// The file a save or reset replaces: `path`, or the file a symbolic link at
/// `path` (as dotfile managers use) points to.
fn link_target(path: &Path) -> Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(resolved) => Ok(resolved),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Writing through a link whose file is gone would replace the
            // link itself and quietly disconnect it from its real location.
            if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink()) {
                bail!("{} is a link to a file that does not exist", path.display());
            }
            Ok(path.to_path_buf())
        }
        Err(error) => Err(error).with_context(|| format!("resolve {}", path.display())),
    }
}

fn read_if_present(path: &Path) -> Result<Option<String>> {
    match crate::safe_fs::read_linked_text(path, super::MAX_CONFIG_FILE_BYTES) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

/// `text`, the file at `path`, as a document to edit and the configuration
/// it holds, if it loads.
fn loadable_document(path: &Path, text: &str) -> Result<(DocumentMut, Config)> {
    Config::from_toml(text, path)
        .and_then(|config| Ok((text.parse::<DocumentMut>()?, config)))
        .map_err(|error| {
            anyhow!(
                "{} has an error, so it was left as it is ({error:#}); fix it, or use Reset Configuration to replace it",
                path.display()
            )
        })
}

/// Change `table`, a table in the person's document, from `baseline` to
/// `live`. A changed value replaces the old one and keeps its comment; a
/// table is changed key by key; an array is replaced whole, and an array of
/// tables keeps what it can of its entries (see [`keep_unchanged_entries`]).
/// What is added comes from `written`, the same table in the live
/// configuration written out; `file` is what `table` holds, as
/// [`config_table`] gives it. `inline` says whether `table` is an inline
/// table, so everything added to it must be inline too.
fn apply_document_changes(
    table: &mut dyn TableLike,
    inline: bool,
    written: &dyn TableLike,
    file: &toml::Table,
    baseline: &toml::Table,
    live: &toml::Table,
) {
    let mut changed = false;
    // New keys go in the order the settings are written in.
    let keys = written
        .iter()
        .map(|(key, _)| key)
        .filter(|key| live.contains_key(*key))
        .chain(
            live.keys()
                .map(String::as_str)
                .filter(|key| !written.contains_key(key)),
        );
    for key in keys {
        let value = &live[key];
        let before = baseline.get(key);
        if before == Some(value) {
            continue;
        }
        changed = true;
        let written_item = written.get(key);
        match (value, before) {
            (toml::Value::Table(live_child), None | Some(toml::Value::Table(_))) => {
                let empty_values = toml::Table::new();
                let empty_written = Table::new();
                let file_child = file
                    .get(key)
                    .and_then(toml::Value::as_table)
                    .unwrap_or(&empty_values);
                let baseline_child = before
                    .and_then(toml::Value::as_table)
                    .unwrap_or(&empty_values);
                let written_child = written_item
                    .and_then(Item::as_table_like)
                    .unwrap_or(&empty_written);
                let created = !table.contains_key(key);
                let item = table.entry(key).or_insert_with(|| new_table(inline));
                if !item.is_table_like() {
                    *item = new_table(inline);
                }
                let child_inline = item.is_inline_table();
                let child = item.as_table_like_mut().expect("the item was made a table");
                apply_document_changes(
                    child,
                    child_inline,
                    written_child,
                    file_child,
                    baseline_child,
                    live_child,
                );
                // A new table with nothing in it still says the entry exists,
                // so its header is written.
                if created
                    && let Some(Item::Table(child)) = table.get_mut(key)
                    && child.is_empty()
                {
                    child.set_implicit(false);
                }
            }
            // A written-out configuration holds every value it has; were one
            // missing, the check after the edit refuses the save.
            _ => {
                if let Some(written_item) = written_item {
                    set_value(table, inline, key, written_item, file.get(key), value);
                }
            }
        }
    }
    for key in baseline.keys().filter(|key| !live.contains_key(*key)) {
        changed |= table.remove(key).is_some();
    }
    if inline && changed {
        // Respace the inline table around what was added or removed.
        table.fmt();
    }
}

/// The same changes as [`apply_document_changes`], made to plain values.
fn apply_value_changes(table: &mut toml::Table, baseline: &toml::Table, live: &toml::Table) {
    for (key, value) in live {
        let before = baseline.get(key);
        if before == Some(value) {
            continue;
        }
        match (value, before) {
            (toml::Value::Table(live_child), None | Some(toml::Value::Table(_))) => {
                let empty = toml::Table::new();
                let baseline_child = before.and_then(toml::Value::as_table).unwrap_or(&empty);
                let child = table
                    .entry(key.as_str())
                    .or_insert_with(|| toml::Value::Table(toml::Table::new()));
                if !child.is_table() {
                    *child = toml::Value::Table(toml::Table::new());
                }
                let toml::Value::Table(child) = child else {
                    unreachable!("the value was made a table");
                };
                apply_value_changes(child, baseline_child, live_child);
            }
            _ => {
                table.insert(key.clone(), value.clone());
            }
        }
    }
    for key in baseline.keys().filter(|key| !live.contains_key(*key)) {
        table.remove(key);
    }
}

fn new_table(inline: bool) -> Item {
    if inline {
        Item::Value(Value::InlineTable(InlineTable::new()))
    } else {
        // A table holding only other tables needs no header of its own.
        let mut table = Table::new();
        table.set_implicit(true);
        Item::Table(table)
    }
}

/// Set `key` in `table` to `written`, whose value is `value`; `in_file` is
/// what the key holds in the file now.
fn set_value(
    table: &mut dyn TableLike,
    inline: bool,
    key: &str,
    written: &Item,
    in_file: Option<&toml::Value>,
    value: &toml::Value,
) {
    let mut item = fresh_item(written);
    match table.get_mut(key) {
        // Keep the person's inline style and the comment after the value.
        Some(existing) if inline || existing.is_value() => {
            if let Ok(mut replacement) = item.into_value() {
                if let Some(old) = existing.as_value() {
                    *replacement.decor_mut() = old.decor().clone();
                }
                *existing = Item::Value(replacement);
            }
        }
        Some(existing) => {
            if let (
                Some(old_tables),
                Some(toml::Value::Array(old_values)),
                toml::Value::Array(new_values),
                Item::ArrayOfTables(new_tables),
            ) = (existing.as_array_of_tables(), in_file, value, &mut item)
            {
                keep_unchanged_entries(old_tables, old_values, new_tables, new_values);
            }
            *existing = item;
        }
        None => {
            if inline {
                item.make_value();
            }
            table.insert(key, item);
        }
    }
}

/// In `new_tables`, the new entries of an array of tables with the values
/// `new_values`, keep what the file's entries, `old_tables` with the values
/// `old_values`, already say. An entry whose value is unchanged is put back
/// as it was written, with its comments, as long as the entries stay in
/// order. Between two kept entries, as many changed entries as there were
/// before are the same entries changed: each is edited key by key, so it too
/// keeps its comments and the keys that did not change. An unchanged entry
/// is found by its value as loaded, never by its position, so one entry is
/// never put back in another's place.
fn keep_unchanged_entries(
    old_tables: &ArrayOfTables,
    old_values: &[toml::Value],
    new_tables: &mut ArrayOfTables,
    new_values: &[toml::Value],
) {
    // Both describe the same document, entry for entry.
    if old_tables.len() != old_values.len() {
        return;
    }
    let mut unchanged = Vec::new();
    let mut next_old = 0;
    for (new_index, value) in new_values.iter().enumerate() {
        if let Some(old_index) =
            (next_old..old_values.len()).find(|&old_index| old_values[old_index] == *value)
        {
            unchanged.push((old_index, new_index));
            next_old = old_index + 1;
        }
    }
    let mut old_start = 0;
    let mut new_start = 0;
    let end = (old_values.len(), new_values.len());
    for (old_end, new_end) in unchanged.iter().copied().chain([end]) {
        if old_end - old_start == new_end - new_start {
            for offset in 0..new_end - new_start {
                let (old_index, new_index) = (old_start + offset, new_start + offset);
                if let Some(old) = old_tables.get(old_index) {
                    edit_entry(
                        old,
                        &old_values[old_index],
                        new_tables,
                        new_index,
                        &new_values[new_index],
                    );
                }
            }
        }
        if let (Some(old), Some(new)) = (old_tables.get(old_end), new_tables.get_mut(new_end)) {
            *new = old.clone();
        }
        old_start = old_end + 1;
        new_start = new_end + 1;
    }
}

/// Replace entry `index` of `new_tables`, whose value is `value`, with `old`,
/// the file's entry that held `old_value`, changed key by key to `value`.
fn edit_entry(
    old: &Table,
    old_value: &toml::Value,
    new_tables: &mut ArrayOfTables,
    index: usize,
    value: &toml::Value,
) {
    let (Some(old_value), Some(value)) = (old_value.as_table(), value.as_table()) else {
        return;
    };
    let Some(written) = new_tables.get(index).cloned() else {
        return;
    };
    let mut edited = old.clone();
    apply_document_changes(&mut edited, false, &written, old_value, old_value, value);
    if let Some(new) = new_tables.get_mut(index) {
        *new = edited;
    }
}

/// `item` from the written-out configuration, without its place and spacing
/// there, so it settles where it is put in the person's document.
fn fresh_item(item: &Item) -> Item {
    match item {
        Item::None => Item::None,
        Item::Value(value) => Item::Value(fresh_value(value)),
        Item::Table(table) => Item::Table(fresh_table(table)),
        Item::ArrayOfTables(tables) => {
            Item::ArrayOfTables(tables.iter().map(fresh_table).collect())
        }
    }
}

fn fresh_table(table: &Table) -> Table {
    let mut fresh = Table::new();
    for (key, item) in table.iter() {
        fresh.insert(key, fresh_item(item));
    }
    fresh
}

fn fresh_value(value: &Value) -> Value {
    match value {
        Value::Array(array) => Value::Array(array.iter().map(fresh_value).collect()),
        Value::InlineTable(table) => Value::InlineTable(
            table
                .iter()
                .map(|(key, value)| (key, fresh_value(value)))
                .collect(),
        ),
        scalar => {
            let mut scalar = scalar.clone();
            scalar.decor_mut().clear();
            scalar
        }
    }
}

/// Replace `target` atomically with `text`, keeping its permissions.
fn replace_file(target: &Path, text: &str) -> Result<()> {
    if let Some(directory) = target.parent() {
        fs::create_dir_all(directory).with_context(|| format!("create {}", directory.display()))?;
    }
    let permissions = fs::metadata(target)
        .ok()
        .map(|metadata| metadata.permissions());
    crate::safe_fs::atomic_replace_keeping(target, permissions, |writer| {
        writer.write_all(text.as_bytes())
    })
    .with_context(|| format!("write {}", target.display()))
}

/// Copy `old`, the open configuration file, beside `path` under the first
/// free name of `<name>.<seconds>.bak`, `<name>.<seconds>-2.bak`, and so on,
/// so a copy never replaces another.
fn keep_copy(path: &Path, mut old: fs::File, now: SystemTime) -> Result<PathBuf> {
    let seconds = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let name = path
        .file_name()
        .with_context(|| format!("{} has no file name", path.display()))?
        .to_string_lossy()
        .into_owned();
    let mode = old
        .metadata()
        .map_or(0o600, |metadata| metadata.permissions().mode() & 0o777);
    for attempt in 1..=MAX_BACKUP_NAMES {
        let suffix = if attempt == 1 {
            seconds.to_string()
        } else {
            format!("{seconds}-{attempt}")
        };
        let backup = path.with_file_name(format!("{name}.{suffix}.bak"));
        let mut file = match crate::safe_fs::create_new(&backup, mode) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("create {}", backup.display()));
            }
        };
        if let Err(error) = std::io::copy(&mut old, &mut file).and_then(|_| file.sync_all()) {
            // A partial copy must not pass for the old file.
            if let Err(removal) = fs::remove_file(&backup) {
                log::warn!(
                    "Could not remove the partial copy {}: {removal}",
                    backup.display()
                );
            }
            return Err(error).with_context(|| format!("write {}", backup.display()));
        }
        return Ok(backup);
    }
    bail!(
        "could not find a free name to keep a copy of {}",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CustomCommand;

    /// Load the file at `path`, change it with `edit`, and save it.
    fn save_edit(path: &Path, edit: impl FnOnce(&mut Config)) -> Result<toml::Table> {
        let mut config = Config::load_from(path).unwrap();
        let baseline = config_table(&config).unwrap();
        edit(&mut config);
        save(path, &baseline, &config)
    }

    fn config_file(text: &str) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, text).unwrap();
        (directory, path)
    }

    fn command(name: &str) -> CustomCommand {
        CustomCommand {
            name: name.into(),
            command: format!("echo {name}"),
            alias: None,
        }
    }

    #[test]
    fn a_save_changes_only_the_changed_setting() {
        let original = "\
# My terminal.
[font]
# A family I like.
family = \"Fira Mono\"   # installed from the distribution
size   = 13

[window]
padding = 4 # tight
";
        let (_directory, path) = config_file(original);
        save_edit(&path, |config| config.window.padding = 12).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            original.replace("padding = 4 # tight", "padding = 12 # tight")
        );
    }

    #[test]
    fn the_shipped_example_keeps_every_line_through_saves() {
        let example = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/config.example.toml"),
        )
        .unwrap();
        let (_directory, path) = config_file(&example);
        save_edit(&path, |config| {
            config.cursor.blink = false;
            config.font.size = 14.0;
            config.commands.push(command("logs"));
            config.agents.insert(
                "codex".into(),
                crate::agents::AgentConfig {
                    enabled: Some(false),
                    ..Default::default()
                },
            );
        })
        .unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        let mut saved_lines = saved.lines();
        let expected = example
            .replace("\nblink = true\n", "\nblink = false\n")
            .replace("\nsize = 12.0 ", "\nsize = 14.0 ");
        assert_ne!(expected, example);
        for line in expected.lines() {
            assert!(
                saved_lines.any(|candidate| candidate == line),
                "{line:?} was lost or moved:\n{saved}"
            );
        }
    }

    #[test]
    fn a_new_setting_goes_in_its_table_or_a_new_one() {
        let original = "[font]\nsize = 13 # big\n\n[cursor]\nblink = false\n";
        let (_directory, path) = config_file(original);
        save_edit(&path, |config| {
            config.font.line_height = 1.5;
            config.window.background_image.opacity = 0.5;
        })
        .unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[font]\nsize = 13 # big\nline_height = 1.5\n\n[cursor]\nblink = false\n\
             \n[window.background_image]\nopacity = 0.5\n"
        );
    }

    #[test]
    fn dotted_keys_and_inline_tables_keep_their_form() {
        let original = "font.size = 13\nwindow = { padding = 4, opacity = 0.9 }\n";
        let (_directory, path) = config_file(original);
        save_edit(&path, |config| {
            config.font.size = 15.0;
            config.font.ligatures = false;
            config.window.padding = 6;
            config.window.background_image.fit = crate::config::BackgroundImageFit::Contain;
        })
        .unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "font.size = 15.0\nfont.ligatures = false\n\
             window = { padding = 6, opacity = 0.9, background_image = { fit = \"contain\" } }\n"
        );
    }

    #[test]
    fn adding_and_deleting_commands_keeps_the_other_entries_as_written() {
        let original = "\
# Hosts I use.
[[commands]]
# The database box.
name = \"db\"
command = \"ssh db\"

[[commands]]
name = \"web\"  # front end
command = \"ssh web\"
";
        let (_directory, path) = config_file(original);
        save_edit(&path, |config| config.commands.push(command("logs"))).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("{original}\n[[commands]]\nname = \"logs\"\ncommand = \"echo logs\"\n")
        );

        save_edit(&path, |config| {
            config.commands.remove(0);
        })
        .unwrap();
        // The deleted entry takes the comments above it along; the kept one
        // keeps the blank line above it.
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "
[[commands]]
name = \"web\"  # front end
command = \"ssh web\"

[[commands]]
name = \"logs\"
command = \"echo logs\"
"
        );
        let names: Vec<_> = Config::load_from(&path)
            .unwrap()
            .commands
            .into_iter()
            .map(|command| command.name)
            .collect();
        assert_eq!(names, ["web", "logs"]);
    }

    #[test]
    fn a_changed_entry_is_edited_in_place_keeping_its_comments() {
        let original = "\
[[commands]]
name = \"db\"
command = \"ssh db\"

# The front end.
[[commands]]
name = \"web\"  # mine
command = \"ssh web\" # old host
";
        let (_directory, path) = config_file(original);
        save_edit(&path, |config| {
            config.commands[1].command = "ssh web2".into();
        })
        .unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            original.replace("\"ssh web\"", "\"ssh web2\"")
        );

        // Removing a key removes its line alone.
        save_edit(&path, |config| {
            config.commands[0].alias = Some("d".into());
        })
        .unwrap();
        save_edit(&path, |config| {
            config.commands[0].alias = None;
        })
        .unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            original.replace("\"ssh web\"", "\"ssh web2\"")
        );
    }

    #[test]
    fn an_entry_is_kept_for_what_it_says_never_for_its_place() {
        use crate::input::keybindings::ShortcutOverride;
        let original = "\
[[shortcuts]]
action = \"toggle_pane_zoom\"
key = \"z\"
ctrl = true

# Alt+Left goes to the shell.
[[shortcuts]]
action = \"navigate_pane_left\"
";
        let (_directory, path) = config_file(original);
        let loaded = Config::load_from(&path).unwrap();
        // A baseline whose entries are in another order than the file's
        // must not put one entry where another was.
        let mut reordered = loaded.clone();
        reordered.shortcuts.reverse();
        let baseline = config_table(&reordered).unwrap();
        let mut live = reordered;
        live.shortcuts.push(ShortcutOverride {
            action: "copy".into(),
            key: Some("k".into()),
            ctrl: true,
            alt: false,
            shift: false,
            super_key: false,
        });
        save(&path, &baseline, &live).unwrap();
        let saved = Config::load_from(&path).unwrap();
        assert_eq!(saved.shortcuts, live.shortcuts);
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.trim_start().starts_with(
                "# Alt+Left goes to the shell.\n[[shortcuts]]\naction = \"navigate_pane_left\"\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn added_keybindings_keep_their_nested_action_with_their_entry() {
        let original = "\
[[keybindings]]
key = \"a\"
action = { type = \"send_text\", text = \"first\" }
";
        let (_directory, path) = config_file(original);
        save_edit(&path, |config| {
            let mut second = config.keybindings[0].clone();
            second.key = "b".into();
            second.action = crate::input::keybindings::KeyAction::RunCommand {
                command: "make".into(),
            };
            config.keybindings.push(second);
        })
        .unwrap();
        let loaded = Config::load_from(&path).unwrap();
        let keys: Vec<_> = loaded
            .keybindings
            .iter()
            .map(|binding| (binding.key.as_str(), format!("{:?}", binding.action)))
            .collect();
        assert_eq!(
            keys,
            [
                ("a", "SendText { text: \"first\" }".to_string()),
                ("b", "RunCommand { command: \"make\" }".to_string())
            ]
        );
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(original), "{text}");
    }

    #[test]
    fn a_setting_cleared_in_termeric_is_removed_from_the_file() {
        let original = "[theme]\nname = \"nord\"\npath = \"/tmp/nord.toml\" # mine\n";
        let (_directory, path) = config_file(original);
        save_edit(&path, |config| config.theme.path = None).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[theme]\nname = \"nord\"\n"
        );
    }

    #[test]
    fn an_edit_made_elsewhere_to_another_setting_survives_a_save() {
        let (_directory, path) = config_file("[window]\npadding = 4\n");
        let mut config = Config::load_from(&path).unwrap();
        let baseline = config_table(&config).unwrap();
        // Another program changes a setting before Termeric saves.
        fs::write(&path, "[window]\npadding = 20\n").unwrap();
        config.cursor.blink = false;
        let file = save(&path, &baseline, &config).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[window]\npadding = 20\n\n[cursor]\nblink = false\n"
        );
        // The caller learns that the file holds more than it saved.
        assert_ne!(file, config_table(&config).unwrap());
        assert_eq!(file["window"]["padding"].as_integer(), Some(20));
    }

    #[test]
    fn a_save_with_nothing_to_change_writes_nothing() {
        let original = "[window]\npadding = 4\n";
        let (directory, path) = config_file(original);
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        save_edit(&path, |_| {}).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);

        // Nor does it create a file that would set nothing.
        let absent = directory.path().join("absent.toml");
        save_edit(&absent, |_| {}).unwrap();
        assert!(!absent.exists());
        // A change to a missing file creates one with just that change.
        save_edit(&absent, |config| config.window.padding = 2).unwrap();
        assert_eq!(
            fs::read_to_string(&absent).unwrap(),
            "[window]\npadding = 2\n"
        );
    }

    #[test]
    fn a_file_that_does_not_load_is_never_saved_over() {
        // One typo away from working: the person's settings must survive.
        for original in [
            "[window]\nwidth = 1234\n[font\nsize = 13\n",
            // So must a file that parses but fails validation.
            "[window]\nwidth = 0\n",
        ] {
            let (_directory, path) = config_file(original);
            let config = Config::default();
            let baseline = config_table(&config).unwrap();
            let mut live = config;
            live.cursor.blink = false;
            let error = save(&path, &baseline, &live).unwrap_err().to_string();
            assert!(error.contains("Reset Configuration"), "{error}");
            assert!(error.contains("config.toml"), "{error}");
            assert_eq!(fs::read_to_string(&path).unwrap(), original);
        }
    }

    #[test]
    fn decimal_settings_are_saved_in_their_shortest_form() {
        let (_directory, path) = config_file("");
        save_edit(&path, |config| {
            config.font.line_height = 1.3;
            config.window.opacity = 0.9;
            config.window.background_image.opacity = 0.35;
        })
        .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        for line in ["line_height = 1.3", "opacity = 0.9", "opacity = 0.35"] {
            assert!(text.lines().any(|candidate| candidate == line), "{text}");
        }
        let defaults = config_table(&Config::default()).unwrap();
        assert_eq!(defaults["font"]["line_height"].as_float(), Some(1.2));
    }

    #[test]
    fn a_reset_keeps_every_old_file_byte_for_byte() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // Nothing to keep yet.
        assert_eq!(reset(&path, now).unwrap(), None);
        assert_eq!(fs::read_to_string(&path).unwrap(), RESET_TEXT);
        Config::load_from(&path).unwrap();

        // A file that loads and one that does not, in the same second.
        let valid = "# Mine.\n[window]\npadding = 4\n";
        let broken = "[window\npadding = 4\n";
        let mut backups = Vec::new();
        for original in [valid, broken] {
            fs::write(&path, original).unwrap();
            let backup = reset(&path, now).unwrap().expect("the old file is kept");
            assert_eq!(fs::read_to_string(&backup).unwrap(), original);
            assert_eq!(fs::read_to_string(&path).unwrap(), RESET_TEXT);
            backups.push(backup);
        }
        assert_eq!(
            backups,
            [
                directory.path().join("config.toml.1700000000.bak"),
                directory.path().join("config.toml.1700000000-2.bak"),
            ]
        );
    }

    /// Reset is how a configuration too large to load is recovered, so it
    /// keeps a copy of any size; a pipe in the file's place is refused
    /// rather than waited on.
    #[test]
    fn a_reset_keeps_a_file_too_large_to_load_and_refuses_a_pipe() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let large = "#".repeat(super::super::MAX_CONFIG_FILE_BYTES + 1);
        fs::write(&path, &large).unwrap();
        assert!(Config::load_from(&path).is_err());
        let backup = reset(&path, SystemTime::now()).unwrap().unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), large);
        assert_eq!(fs::read_to_string(&path).unwrap(), RESET_TEXT);

        fs::remove_file(&path).unwrap();
        let fifo = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: `fifo` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(reset(&path, SystemTime::now()).is_err());
        assert!(std::os::unix::fs::FileTypeExt::is_fifo(
            &fs::symlink_metadata(&path).unwrap().file_type()
        ));
    }

    #[test]
    fn a_reset_keeps_the_old_file_with_its_own_permissions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        for mode in [0o644, 0o600, 0o666] {
            fs::write(&path, "[window]\npadding = 4\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            let backup = reset(&path, SystemTime::now()).unwrap().unwrap();
            assert_eq!(
                fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
                mode
            );
        }
    }

    #[test]
    fn a_reset_through_a_symbolic_link_keeps_the_link() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let real = directory.path().join("termeric.toml");
        fs::write(&real, "[window]\npadding = 4\n").unwrap();
        let path = directory.path().join("config.toml");
        symlink(&real, &path).unwrap();
        let backup = reset(&path, SystemTime::now()).unwrap().unwrap();
        assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), RESET_TEXT);
        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            "[window]\npadding = 4\n"
        );
        assert_eq!(backup.parent(), path.parent());
    }

    #[test]
    fn a_symbolic_link_is_followed_and_kept_with_the_file_permissions() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let dotfiles = directory.path().join("dotfiles");
        fs::create_dir(&dotfiles).unwrap();
        let real = dotfiles.join("termeric.toml");
        fs::write(&real, "[window]\npadding = 4\n").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o644)).unwrap();
        let path = directory.path().join("config.toml");
        symlink(&real, &path).unwrap();

        save_edit(&path, |config| config.window.padding = 9).unwrap();
        assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
        assert_eq!(
            fs::read_to_string(&real).unwrap(),
            "[window]\npadding = 9\n"
        );
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o644
        );

        fs::remove_file(&real).unwrap();
        let config = Config::default();
        let baseline = config_table(&config).unwrap();
        let mut live = config;
        live.window.padding = 3;
        let error = save(&path, &baseline, &live).unwrap_err();
        assert!(error.to_string().contains("does not exist"), "{error}");
        assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
    }
}
