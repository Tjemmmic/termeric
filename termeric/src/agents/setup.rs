//! Installing and removing the hooks through which agents report their state.
//!
//! Claude Code reads hooks from its `settings.json` and Codex from its
//! `hooks.json`, both shaped as `{"hooks": {Event: [group, ...]}}`. Termeric
//! appends one group per event that runs `termeric agent-event AGENT EVENT`,
//! and recognizes its own groups by that command. Everything else in the
//! file keeps its exact text and order. The hook command does nothing outside
//! a Termeric terminal, so installing it is harmless for other terminals.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use super::hook::HookEvent;

/// Seconds an agent waits for one Termeric hook. The command itself gives
/// up well before this.
const HOOK_TIMEOUT_SECONDS: u64 = 5;
/// Settings files are small; anything larger is not one Termeric should edit.
const MAX_SETTINGS_BYTES: u64 = 4 * 1024 * 1024;

/// An agent whose hooks Termeric knows how to install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookedAgent {
    pub agent_id: &'static str,
    /// Each agent event, the matcher that narrows it, and what it reports.
    events: &'static [(&'static str, Option<&'static str>, HookEvent)],
}

pub const CLAUDE_CODE: HookedAgent = HookedAgent {
    agent_id: "claude-code",
    events: &[
        ("SessionStart", None, HookEvent::SessionStart),
        ("UserPromptSubmit", None, HookEvent::Prompt),
        ("PostToolUse", None, HookEvent::ToolDone),
        (
            "Notification",
            Some("permission_prompt|elicitation_dialog"),
            HookEvent::NeedsInput,
        ),
        ("Stop", None, HookEvent::Stop),
        ("StopFailure", None, HookEvent::StopFailure),
        ("SessionEnd", None, HookEvent::SessionEnd),
    ],
};

pub const CODEX: HookedAgent = HookedAgent {
    agent_id: "codex",
    events: &[
        ("SessionStart", None, HookEvent::SessionStart),
        ("UserPromptSubmit", None, HookEvent::Prompt),
        ("PostToolUse", None, HookEvent::ToolDone),
        ("PermissionRequest", None, HookEvent::NeedsInput),
        ("Stop", None, HookEvent::Stop),
        ("SessionEnd", None, HookEvent::SessionEnd),
    ],
};

pub const HOOKED_AGENTS: [HookedAgent; 2] = [CLAUDE_CODE, CODEX];

impl HookedAgent {
    /// The file this agent reads hooks from, honoring the agent's own
    /// configuration-directory variable.
    pub fn settings_path(self) -> Option<PathBuf> {
        let (variable, default, file) = match self.agent_id {
            "claude-code" => ("CLAUDE_CONFIG_DIR", ".claude", "settings.json"),
            _ => ("CODEX_HOME", ".codex", "hooks.json"),
        };
        let directory = std::env::var_os(variable)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(default)))?;
        Some(directory.join(file))
    }
}

/// How an agent's hooks compare with what Termeric would install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookState {
    NotInstalled,
    Installed,
    /// Installed for another Termeric executable or an older event set.
    Outdated,
}

/// The shell command a hook runs. The path is left unquoted when it is
/// plainly safe, so the command also works in shells without POSIX quoting.
pub fn hook_command(executable: &Path, agent_id: &str, event: HookEvent) -> String {
    let path = executable.to_string_lossy();
    let plain = path
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/._-+".contains(&byte));
    let program = if plain {
        path.into_owned()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    };
    format!("{program} agent-event {agent_id} {}", event.name())
}

/// Whether a hook group is one Termeric installed for `agent_id`.
fn is_termeric_group(group: &Value, agent_id: &str) -> bool {
    let marker = format!(" agent-event {agent_id} ");
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            !hooks.is_empty()
                && hooks.iter().all(|hook| {
                    hook.get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|command| {
                            command.contains("termeric") && command.contains(&marker)
                        })
                })
        })
}

fn termeric_group(
    agent: HookedAgent,
    executable: &Path,
    event: HookEvent,
    matcher: Option<&str>,
) -> Value {
    let mut group = json!({
        "hooks": [{
            "type": "command",
            "command": hook_command(executable, agent.agent_id, event),
            "timeout": HOOK_TIMEOUT_SECONDS,
        }]
    });
    if let Some(matcher) = matcher {
        group["matcher"] = json!(matcher);
    }
    group
}

/// A settings object as its top-level entries, each kept as the exact text
/// it had in the file.
struct Entries(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for Entries {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Entries;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a JSON object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Entries, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry::<String, Box<RawValue>>()? {
                    entries.push(entry);
                }
                Ok(Entries(entries))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

fn parse_entries(text: &str) -> Result<Entries> {
    if text.trim().is_empty() {
        return Ok(Entries(Vec::new()));
    }
    serde_json::from_str(text).context("the file is not a JSON object")
}

fn hooks_value(entries: &Entries) -> Result<Value> {
    match entries.0.iter().find(|(key, _)| key == "hooks") {
        None => Ok(json!({})),
        Some((_, raw)) => {
            let hooks: Value = serde_json::from_str(raw.get())?;
            anyhow::ensure!(hooks.is_object(), "\"hooks\" is not an object");
            Ok(hooks)
        }
    }
}

/// How the agent's current hooks compare with Termeric's.
pub fn hook_state(agent: HookedAgent, text: &str, executable: &Path) -> Result<HookState> {
    let hooks = hooks_value(&parse_entries(text)?)?;
    let groups = |event: &str| {
        hooks
            .get(event)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter(|group| is_termeric_group(group, agent.agent_id))
            .collect::<Vec<_>>()
    };
    let any = hooks
        .as_object()
        .is_some_and(|events| events.keys().any(|event| !groups(event).is_empty()));
    if !any {
        return Ok(HookState::NotInstalled);
    }
    let current = agent.events.iter().all(|(event, matcher, hook)| {
        groups(event) == [&termeric_group(agent, executable, *hook, *matcher)]
    });
    Ok(if current {
        HookState::Installed
    } else {
        HookState::Outdated
    })
}

/// The file text with Termeric's hooks for `agent` replaced by the current
/// set (`install`) or removed. Other hooks and settings are untouched.
pub fn rewrite(agent: HookedAgent, text: &str, executable: &Path, install: bool) -> Result<String> {
    let mut entries = parse_entries(text)?;
    let mut hooks = hooks_value(&entries)?;
    let events = hooks
        .as_object_mut()
        .expect("hooks were checked to be an object");
    for groups in events.values_mut() {
        if let Some(groups) = groups.as_array_mut() {
            groups.retain(|group| !is_termeric_group(group, agent.agent_id));
        }
    }
    events.retain(|_, groups| groups.as_array().is_none_or(|groups| !groups.is_empty()));
    if install {
        for (event, matcher, hook) in agent.events {
            let groups = events.entry(*event).or_insert_with(|| json!([]));
            let groups = groups
                .as_array_mut()
                .with_context(|| format!("\"hooks\".{event:?} is not a list"))?;
            // Appended last: Codex trusts hooks by position, so existing
            // groups keep theirs.
            groups.push(termeric_group(agent, executable, *hook, *matcher));
        }
    }
    let indent = indentation(text);
    let hooks_text = if events.is_empty() {
        None
    } else {
        let mut bytes = Vec::new();
        let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
        serde::Serialize::serialize(
            &hooks,
            &mut serde_json::Serializer::with_formatter(&mut bytes, formatter),
        )?;
        // One level in, beneath its key.
        let nested = String::from_utf8(bytes)?.replace('\n', &format!("\n{indent}"));
        Some(RawValue::from_string(nested)?)
    };
    match (
        entries.0.iter().position(|(key, _)| key == "hooks"),
        hooks_text,
    ) {
        (Some(index), Some(text)) => entries.0[index].1 = text,
        (Some(index), None) => {
            entries.0.remove(index);
        }
        (None, Some(text)) => entries.0.push(("hooks".into(), text)),
        (None, None) => {}
    }
    Ok(render(&entries, &indent))
}

/// The file's own indentation, taken from its first entry; two spaces for a
/// new or single-line file.
fn indentation(text: &str) -> String {
    let Some(open) = text.find('{') else {
        return "  ".into();
    };
    let rest = &text[open + 1..];
    let Some(line_start) = rest.find('\n') else {
        return "  ".into();
    };
    let indent: String = rest[line_start + 1..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    if indent.is_empty() {
        "  ".into()
    } else {
        indent
    }
}

/// The entries as a JSON object, one per line. Untouched entries keep their
/// original text, which already carries its own nested indentation.
fn render(entries: &Entries, indent: &str) -> String {
    if entries.0.is_empty() {
        return "{}\n".into();
    }
    let body = entries
        .0
        .iter()
        .map(|(key, value)| {
            let key = serde_json::to_string(key).expect("strings serialize");
            format!("{indent}{key}: {}", value.get())
        })
        .collect::<Vec<_>>()
        .join(",\n");
    format!("{{\n{body}\n}}\n")
}

/// Read a settings file, treating a missing one as empty.
pub fn read_settings(path: &Path) -> Result<String> {
    match fs::metadata(path) {
        Ok(metadata) => {
            anyhow::ensure!(metadata.is_file(), "{} is not a file", path.display());
            anyhow::ensure!(
                metadata.len() <= MAX_SETTINGS_BYTES,
                "{} is too large to edit",
                path.display()
            );
            fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

/// Replace a settings file whose text is still `previous`. Returns
/// `Ok(false)`, writing nothing, when it has changed since it was read, so an
/// edit made meanwhile is never lost.
pub fn replace_if_unchanged(path: &Path, previous: &str, text: &str) -> Result<bool> {
    if read_settings(path)? != previous {
        return Ok(false);
    }
    write_settings(path, previous, text)?;
    Ok(true)
}

/// Replace a settings file atomically, keeping the previous text beside it
/// as `NAME.termeric-backup`. A symbolic link (as dotfile managers use) is
/// followed, and the file it points to is replaced in place.
fn write_settings(path: &Path, previous: &str, text: &str) -> Result<()> {
    let target = match fs::canonicalize(path) {
        Ok(resolved) => resolved,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(),
        Err(error) => return Err(error).with_context(|| format!("resolve {}", path.display())),
    };
    let directory = target
        .parent()
        .with_context(|| format!("{} has no directory", target.display()))?;
    fs::create_dir_all(directory).with_context(|| format!("create {}", directory.display()))?;
    let name = target
        .file_name()
        .with_context(|| format!("{} has no file name", target.display()))?
        .to_string_lossy()
        .into_owned();
    let permissions = fs::metadata(&target)
        .ok()
        .map(|metadata| metadata.permissions());
    if !previous.is_empty() {
        replace(
            &directory.join(format!("{name}.termeric-backup")),
            previous,
            permissions.clone(),
        )?;
    }
    replace(&target, text, permissions)
}

fn replace(path: &Path, text: &str, permissions: Option<fs::Permissions>) -> Result<()> {
    let temporary = path.with_file_name(format!(
        ".{}.termeric-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe() -> PathBuf {
        PathBuf::from("/usr/bin/termeric")
    }

    #[test]
    fn commands_quote_only_unusual_paths() {
        assert_eq!(
            hook_command(&exe(), "codex", HookEvent::Stop),
            "/usr/bin/termeric agent-event codex stop"
        );
        assert_eq!(
            hook_command(
                Path::new("/opt/my apps/it's/termeric"),
                "codex",
                HookEvent::Prompt
            ),
            "'/opt/my apps/it'\\''s/termeric' agent-event codex prompt"
        );
    }

    #[test]
    fn installing_keeps_other_settings_verbatim_and_in_order() {
        let original = r#"{
  "zeta": {"b": 1, "a": 2},
  "hooks": {
    "Stop": [
      {"hooks": [{"type": "command", "command": "notify-me"}]}
    ]
  },
  "alpha": [1, 2,   3]
}
"#;
        let installed = rewrite(CLAUDE_CODE, original, &exe(), true).unwrap();
        assert!(installed.starts_with("{\n  \"zeta\": {\"b\": 1, \"a\": 2},\n"));
        assert!(installed.contains("\"alpha\": [1, 2,   3]"));
        let value: Value = serde_json::from_str(&installed).unwrap();
        let stop = value["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "the person's own hook stays first");
        assert_eq!(stop[0]["hooks"][0]["command"], "notify-me");
        assert_eq!(
            stop[1]["hooks"][0]["command"],
            "/usr/bin/termeric agent-event claude-code stop"
        );
        assert_eq!(
            value["hooks"]["Notification"][0]["matcher"],
            "permission_prompt|elicitation_dialog"
        );
        assert_eq!(
            hook_state(CLAUDE_CODE, &installed, &exe()).unwrap(),
            HookState::Installed
        );
        assert_eq!(
            hook_state(CODEX, &installed, &exe()).unwrap(),
            HookState::NotInstalled
        );

        // Installing again changes nothing; another executable is outdated.
        assert_eq!(
            rewrite(CLAUDE_CODE, &installed, &exe(), true).unwrap(),
            installed
        );
        assert_eq!(
            hook_state(CLAUDE_CODE, &installed, Path::new("/elsewhere/termeric")).unwrap(),
            HookState::Outdated
        );

        let removed = rewrite(CLAUDE_CODE, &installed, &exe(), false).unwrap();
        let value: Value = serde_json::from_str(&removed).unwrap();
        assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(value["hooks"].get("SessionStart").is_none());
        assert!(removed.contains("\"alpha\": [1, 2,   3]"));
        assert_eq!(
            hook_state(CLAUDE_CODE, &removed, &exe()).unwrap(),
            HookState::NotInstalled
        );
    }

    #[test]
    fn rewritten_hooks_follow_the_files_indentation() {
        let four = "{\n    \"model\": \"opus\",\n    \"env\": {\n        \"A\": \"1\"\n    }\n}\n";
        let installed = rewrite(CLAUDE_CODE, four, &exe(), true).unwrap();
        assert!(installed.contains("    \"env\": {\n        \"A\": \"1\"\n    },\n"));
        assert!(installed.contains("    \"hooks\": {\n        \"Notification\": [\n"));
        let removed = rewrite(CLAUDE_CODE, &installed, &exe(), false).unwrap();
        assert_eq!(removed, four, "removal restores the original text");
    }

    #[test]
    fn missing_files_gain_only_hooks_and_lose_them_cleanly() {
        let installed = rewrite(CODEX, "", &exe(), true).unwrap();
        let value: Value = serde_json::from_str(&installed).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 1);
        assert_eq!(
            value["hooks"]["PermissionRequest"][0]["hooks"][0]["command"],
            "/usr/bin/termeric agent-event codex needs-input"
        );
        assert_eq!(rewrite(CODEX, &installed, &exe(), false).unwrap(), "{}\n");
    }

    #[test]
    fn malformed_settings_are_refused_not_rewritten() {
        for text in [
            "[1, 2]",
            "{\"hooks\": []}",
            "{\"hooks\": {\"Stop\": {}}}",
            "{oops",
        ] {
            assert!(rewrite(CLAUDE_CODE, text, &exe(), true).is_err(), "{text}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn writes_follow_links_keep_permissions_and_back_up() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let dotfiles = temp.path().join("dotfiles");
        fs::create_dir(&dotfiles).unwrap();
        let real = dotfiles.join("settings.json");
        fs::write(&real, "{\"a\": 1}\n").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let link = temp.path().join("settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let previous = read_settings(&link).unwrap();
        let next = rewrite(CLAUDE_CODE, &previous, &exe(), true).unwrap();
        write_settings(&link, &previous, &next).unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&real).unwrap(), next);
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::read_to_string(dotfiles.join("settings.json.termeric-backup")).unwrap(),
            previous
        );
        assert_eq!(
            read_settings(&temp.path().join("missing.json")).unwrap(),
            ""
        );

        // An edit made after reading is kept, and nothing is written.
        fs::write(&real, "{\"edited\": true}\n").unwrap();
        assert!(!replace_if_unchanged(&link, &next, "{}\n").unwrap());
        assert_eq!(fs::read_to_string(&real).unwrap(), "{\"edited\": true}\n");
        assert!(replace_if_unchanged(&link, "{\"edited\": true}\n", "{}\n").unwrap());
        assert_eq!(fs::read_to_string(&real).unwrap(), "{}\n");
    }
}
