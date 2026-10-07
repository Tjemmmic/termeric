//! Installing and removing the hooks through which agents report their state.
//!
//! Claude Code reads hooks from its `settings.json` and Codex from its
//! `hooks.json`, both shaped as `{"hooks": {Event: [group, ...]}}`. Termeric
//! appends one group per event that runs `termeric agent-event AGENT EVENT`,
//! and recognizes its own groups by that command. Everything else in the
//! file keeps its exact text and order. The hook command does nothing outside
//! a Termeric terminal, so installing it is harmless for other terminals.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use super::hook::HookEvent;

/// Seconds an agent waits for one Termeric hook. The command itself gives
/// up well before this.
const HOOK_TIMEOUT_SECONDS: u64 = 5;
/// Codex caps hooks that run while a session closes or a turn is interrupted
/// at three seconds, and warns at every start about a longer one.
const SHORT_HOOK_TIMEOUT_SECONDS: u64 = 2;
/// Settings files are small; anything larger is not one Termeric should edit.
const MAX_SETTINGS_BYTES: usize = 4 * 1024 * 1024;

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
        // A tool that fails runs this instead of `PostToolUse`.
        ("PostToolUseFailure", None, HookEvent::ToolDone),
        (
            "Notification",
            Some("permission_prompt|elicitation_dialog|elicitation_url_dialog"),
            HookEvent::NeedsInput,
        ),
        // Claude Code runs no hook when a turn is interrupted; it notifies
        // once it has sat at its prompt for a while.
        ("Notification", Some("idle_prompt"), HookEvent::Stopped),
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
        ("Interrupt", None, HookEvent::Stopped),
        ("SessionEnd", None, HookEvent::SessionEnd),
    ],
};

pub const HOOKED_AGENTS: [HookedAgent; 2] = [CLAUDE_CODE, CODEX];

impl HookedAgent {
    /// This agent's own configuration directory, honoring its directory
    /// variable as set for it in Termeric's configuration (`launch_env`),
    /// else in Termeric's own environment. A relative value is ignored: it
    /// would depend on where the agent starts.
    pub fn config_dir(self, launch_env: &BTreeMap<String, String>) -> Option<PathBuf> {
        let (variable, default) = match self.agent_id {
            "claude-code" => ("CLAUDE_CONFIG_DIR", ".claude"),
            _ => ("CODEX_HOME", ".codex"),
        };
        launch_env
            .get(variable)
            .map(PathBuf::from)
            .or_else(|| std::env::var_os(variable).map(PathBuf::from))
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(default)))
    }

    /// The file this agent reads hooks from, in its configuration directory.
    pub fn settings_path(self, launch_env: &BTreeMap<String, String>) -> Option<PathBuf> {
        let file = match self.agent_id {
            "claude-code" => "settings.json",
            _ => "hooks.json",
        };
        Some(self.config_dir(launch_env)?.join(file))
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
    event_name: &str,
    event: HookEvent,
    matcher: Option<&str>,
) -> Value {
    let timeout = match event_name {
        "SessionEnd" | "Interrupt" => SHORT_HOOK_TIMEOUT_SECONDS,
        _ => HOOK_TIMEOUT_SECONDS,
    };
    let mut group = json!({
        "hooks": [{
            "type": "command",
            "command": hook_command(executable, agent.agent_id, event),
            "timeout": timeout,
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
                let mut entries: Vec<(String, Box<RawValue>)> = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Box<RawValue>>()? {
                    // Parsers disagree on which duplicate wins, so an edit to
                    // one could be ignored; refuse instead of guessing.
                    if entries.iter().any(|(existing, _)| *existing == key) {
                        return Err(serde::de::Error::custom(format!(
                            "the key {key:?} appears more than once"
                        )));
                    }
                    entries.push((key, value));
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
    // Every event must hold exactly the groups Termeric would install now,
    // in order, and no event may hold groups Termeric no longer installs.
    let expected = |event: &str| {
        agent
            .events
            .iter()
            .filter(|(name, ..)| *name == event)
            .map(|(name, matcher, hook)| termeric_group(agent, executable, name, *hook, *matcher))
            .collect::<Vec<_>>()
    };
    let current = hooks.as_object().is_some_and(|events| {
        events
            .keys()
            .map(String::as_str)
            .chain(agent.events.iter().map(|(name, ..)| *name))
            .all(|event| groups(event).into_iter().cloned().collect::<Vec<_>>() == expected(event))
    });
    Ok(if current {
        HookState::Installed
    } else {
        HookState::Outdated
    })
}

/// The file text with Termeric's hooks for `agent` replaced by the current
/// set (`install`) or removed. Everything else keeps its exact text and
/// order: other settings, other hook events, and the person's own groups
/// within an event Termeric also hooks.
pub fn rewrite(agent: HookedAgent, text: &str, executable: &Path, install: bool) -> Result<String> {
    let mut entries = parse_entries(text)?;
    let indent = indentation(text);
    let hooks_index = entries.0.iter().position(|(key, _)| key == "hooks");
    let original: Entries = match hooks_index {
        Some(index) => {
            serde_json::from_str(entries.0[index].1.get()).context("\"hooks\" is not an object")?
        }
        None => Entries(Vec::new()),
    };
    let ours = |event: &str| -> Vec<Value> {
        agent
            .events
            .iter()
            .filter(|(name, ..)| install && *name == event)
            .map(|(name, matcher, hook)| termeric_group(agent, executable, name, *hook, *matcher))
            .collect()
    };
    let mut changed = false;
    let mut events: Vec<(String, Box<RawValue>)> = Vec::new();
    for (event, raw) in original.0 {
        let groups: Vec<Box<RawValue>> = serde_json::from_str(raw.get())
            .with_context(|| format!("\"hooks\".{event:?} is not a list"))?;
        let mut added = Some(ours(&event)).filter(|added| !added.is_empty());
        if added.is_none() && !groups.iter().any(|group| is_termeric_raw(group, agent)) {
            events.push((event, raw));
            continue;
        }
        changed = true;
        // Termeric's groups take the place of its earlier ones, else go
        // last. Codex keys each hook's trust by its position in the list, so
        // the person's hooks must not move.
        let mut items = Vec::new();
        for group in &groups {
            if !is_termeric_raw(group, agent) {
                items.push(group.get().to_owned());
            } else if let Some(added) = added.take() {
                for group in &added {
                    items.push(render_group(group, &indent)?);
                }
            }
        }
        for group in added.iter().flatten() {
            items.push(render_group(group, &indent)?);
        }
        if !items.is_empty() {
            events.push((event, render_list(&items, &indent)?));
        }
    }
    for (event, ..) in agent.events {
        if install && !events.iter().any(|(name, _)| name == event) {
            changed = true;
            let items = ours(event)
                .iter()
                .map(|group| render_group(group, &indent))
                .collect::<Result<Vec<_>>>()?;
            events.push(((*event).to_owned(), render_list(&items, &indent)?));
        }
    }
    if changed {
        let hooks = (!events.is_empty())
            .then(|| render_object(&events, &indent, 2))
            .transpose()?;
        match (hooks_index, hooks) {
            (Some(index), Some(hooks)) => entries.0[index].1 = hooks,
            (Some(index), None) => {
                entries.0.remove(index);
            }
            (None, Some(hooks)) => entries.0.push(("hooks".into(), hooks)),
            (None, None) => {}
        }
    }
    Ok(render(&entries, &indent))
}

/// Whether a group, as written in the file, is one Termeric installed.
fn is_termeric_raw(group: &RawValue, agent: HookedAgent) -> bool {
    serde_json::from_str::<Value>(group.get())
        .is_ok_and(|group| is_termeric_group(&group, agent.agent_id))
}

/// One of Termeric's groups as JSON text for nesting depth three.
fn render_group(group: &Value, indent: &str) -> Result<String> {
    let mut bytes = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    serde::Serialize::serialize(
        group,
        &mut serde_json::Serializer::with_formatter(&mut bytes, formatter),
    )?;
    Ok(String::from_utf8(bytes)?.replace('\n', &format!("\n{}", indent.repeat(3))))
}

/// An event's list of groups, already JSON text, each on its own line at
/// nesting depth three.
fn render_list(items: &[String], indent: &str) -> Result<Box<RawValue>> {
    let inner = indent.repeat(3);
    let body = items
        .iter()
        .map(|item| format!("{inner}{item}"))
        .collect::<Vec<_>>()
        .join(",\n");
    Ok(RawValue::from_string(format!(
        "[\n{body}\n{}]",
        indent.repeat(2)
    ))?)
}

/// An object whose values are already JSON text, with its keys at `depth`.
fn render_object(
    entries: &[(String, Box<RawValue>)],
    indent: &str,
    depth: usize,
) -> Result<Box<RawValue>> {
    let inner = indent.repeat(depth);
    let body = entries
        .iter()
        .map(|(key, value)| {
            let key = serde_json::to_string(key).expect("strings serialize");
            format!("{inner}{key}: {}", value.get())
        })
        .collect::<Vec<_>>()
        .join(",\n");
    Ok(RawValue::from_string(format!(
        "{{\n{body}\n{}}}",
        indent.repeat(depth - 1)
    ))?)
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
    match crate::safe_fs::read_linked_text(path, MAX_SETTINGS_BYTES) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
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
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Writing through a link whose file is gone would replace the
            // link itself and quietly disconnect it from its real location.
            anyhow::ensure!(
                !fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink()),
                "{} is a link to a file that does not exist",
                path.display()
            );
            path.to_path_buf()
        }
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
        let backup = directory.join(format!("{name}.termeric-backup"));
        crate::safe_fs::atomic_replace_keeping(&backup, permissions.clone(), |writer| {
            writer.write_all(previous.as_bytes())
        })
        .with_context(|| format!("write {}", backup.display()))?;
    }
    crate::safe_fs::atomic_replace_keeping(&target, permissions, |writer| {
        writer.write_all(text.as_bytes())
    })
    .with_context(|| format!("write {}", target.display()))
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
        let notification = &value["hooks"]["Notification"];
        assert_eq!(
            notification[0]["matcher"],
            "permission_prompt|elicitation_dialog|elicitation_url_dialog"
        );
        assert_eq!(notification[1]["matcher"], "idle_prompt");
        assert_eq!(
            notification[1]["hooks"][0]["command"],
            "/usr/bin/termeric agent-event claude-code stopped"
        );
        assert_eq!(value["hooks"]["Stop"][1]["hooks"][0]["timeout"], 5);
        assert_eq!(value["hooks"]["SessionEnd"][0]["hooks"][0]["timeout"], 2);
        // An older install, before the idle hook existed, needs updating.
        let mut older = value.clone();
        older["hooks"]["Notification"].as_array_mut().unwrap().pop();
        assert_eq!(
            hook_state(CLAUDE_CODE, &older.to_string(), &exe()).unwrap(),
            HookState::Outdated
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
    fn the_persons_own_hooks_keep_their_exact_text_and_order() {
        let original = r#"{
  "hooks": {
    "UserPromptSubmit": [ {"hooks": [{"type": "command", "command": "log-prompt", "timeout": 3}]} ],
    "Stop": [
      {"matcher": "", "hooks": [{"type": "command", "command": "say done"}]}
    ],
    "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "guard"}]}]
  },
  "model": "opus"
}
"#;
        let installed = rewrite(CLAUDE_CODE, original, &exe(), true).unwrap();
        // Events Termeric does not hook are copied byte for byte.
        assert!(installed.contains(
            r#""PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "guard"}]}]"#
        ));
        // The person's groups in shared events keep their text, and come first.
        assert!(installed.contains(
            r#"{"hooks": [{"type": "command", "command": "log-prompt", "timeout": 3}]}"#
        ));
        assert!(
            installed.contains(
                r#"{"matcher": "", "hooks": [{"type": "command", "command": "say done"}]}"#
            )
        );
        let order: Vec<_> = serde_json::from_str::<Value>(&installed).unwrap()["hooks"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(order.len(), 9);
        // Only a round trip through Termeric's own format could reorder
        // keys; the person's events stay where they were.
        let position = |needle: &str| installed.find(needle).unwrap();
        assert!(position("\"UserPromptSubmit\"") < position("\"Stop\""));
        assert!(position("\"Stop\"") < position("\"PreToolUse\""));
        assert_eq!(
            hook_state(CLAUDE_CODE, &installed, &exe()).unwrap(),
            HookState::Installed
        );
        let removed = rewrite(CLAUDE_CODE, &installed, &exe(), false).unwrap();
        let reparsed: Value = serde_json::from_str(&removed).unwrap();
        assert_eq!(reparsed, serde_json::from_str::<Value>(original).unwrap());
        assert!(removed.contains(
            r#""PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "guard"}]}]"#
        ));
        // Installing on top of an up-to-date install changes nothing.
        assert_eq!(
            rewrite(CLAUDE_CODE, &installed, &exe(), true).unwrap(),
            installed
        );
    }

    #[test]
    fn the_agents_configured_directory_decides_where_hooks_go() {
        let launch_env = BTreeMap::from([(
            "CLAUDE_CONFIG_DIR".to_string(),
            "/srv/claude-work".to_string(),
        )]);
        assert_eq!(
            CLAUDE_CODE.settings_path(&launch_env),
            Some(PathBuf::from("/srv/claude-work/settings.json"))
        );
        // A relative directory would depend on where the agent starts.
        let relative = BTreeMap::from([("CODEX_HOME".to_string(), "codex".to_string())]);
        assert!(
            CODEX
                .settings_path(&relative)
                .is_none_or(|path| path.is_absolute() && path.ends_with("hooks.json"))
        );
    }

    #[test]
    fn updating_leaves_every_group_where_it_was() {
        // Termeric's group first, then one the person added after setup.
        let installed = rewrite(CODEX, "", &exe(), true).unwrap();
        let mut value: Value = serde_json::from_str(&installed).unwrap();
        value["hooks"]["Stop"]
            .as_array_mut()
            .unwrap()
            .push(json!({"hooks": [{"type": "command", "command": "say done"}]}));
        let with_theirs = serde_json::to_string_pretty(&value).unwrap();
        // A newer Termeric replaces its group in place.
        let moved = with_theirs.replace(
            "/usr/bin/termeric agent-event codex stop",
            "/old/termeric agent-event codex stop",
        );
        let updated = rewrite(CODEX, &moved, &exe(), true).unwrap();
        let stop = &serde_json::from_str::<Value>(&updated).unwrap()["hooks"]["Stop"];
        assert_eq!(
            stop[0]["hooks"][0]["command"],
            "/usr/bin/termeric agent-event codex stop"
        );
        assert_eq!(stop[1]["hooks"][0]["command"], "say done");
        assert_eq!(stop.as_array().unwrap().len(), 2);
    }

    #[test]
    fn duplicate_keys_are_refused() {
        let doubled = "{\"hooks\": {}, \"hooks\": {\"Stop\": []}}";
        assert!(rewrite(CLAUDE_CODE, doubled, &exe(), true).is_err());
        let doubled_event = "{\"hooks\": {\"Stop\": [], \"Stop\": []}}";
        assert!(rewrite(CLAUDE_CODE, doubled_event, &exe(), true).is_err());
    }

    #[test]
    fn rewritten_hooks_follow_the_files_indentation() {
        let four = "{\n    \"model\": \"opus\",\n    \"env\": {\n        \"A\": \"1\"\n    }\n}\n";
        let installed = rewrite(CLAUDE_CODE, four, &exe(), true).unwrap();
        assert!(installed.contains("    \"env\": {\n        \"A\": \"1\"\n    },\n"));
        let first_event = format!(
            "    \"hooks\": {{\n        \"{}\": [\n            {{\n                \"",
            CLAUDE_CODE.events[0].0
        );
        assert!(installed.contains(&first_event), "{installed}");
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

        // A link whose file is gone is left alone rather than replaced.
        let dangling = temp.path().join("dangling.json");
        std::os::unix::fs::symlink(dotfiles.join("gone.json"), &dangling).unwrap();
        assert!(write_settings(&dangling, "", "{}\n").is_err());
        assert!(fs::symlink_metadata(&dangling).unwrap().is_symlink());
        assert!(!dotfiles.join("gone.json").exists());
    }

    #[test]
    fn new_settings_files_are_private_and_leave_no_staging_behind() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agent").join("settings.json");
        let text = rewrite(CLAUDE_CODE, "", &exe(), true).unwrap();
        assert!(replace_if_unchanged(&path, "", &text).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        // Settings can hold API keys; a new file never takes the umask's
        // world-readable default.
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);
        let entries: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, ["settings.json"]);
    }
}
