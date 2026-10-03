use super::{effective_env, upsert_env};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const TRANSPORT: &str = "TERMERIC_ELVISH_INTEGRATION_XDG_DATA_DIRS";
const HAD: &str = "TERMERIC_ELVISH_HAD_XDG_DATA_DIRS";
const ORIGINAL: &str = "TERMERIC_ELVISH_ORIGINAL_XDG_DATA_DIRS";

pub(super) fn configure(base: &Path, env: &mut Vec<(String, String)>) -> Result<()> {
    let current = effective_env(env, "XDG_DATA_DIRS");
    let recorded = effective_env(env, TRANSPORT);
    let saved = match effective_env(env, HAD).as_deref() {
        Some("0") => Some(None),
        Some("1") => effective_env(env, ORIGINAL).map(Some),
        _ => None,
    };
    // Unwrap only our own transport; a profile's different path is the user's.
    let is_transport = recorded.is_some_and(|value| !value.is_empty() && current == Some(value));
    let original = match saved {
        Some(saved) if is_transport => saved,
        _ => current,
    };
    let joined = search_path(base, original.as_deref())?;
    upsert_env(env, HAD, if original.is_some() { "1" } else { "0" }.into());
    upsert_env(env, ORIGINAL, original.unwrap_or_default());
    upsert_env(env, TRANSPORT, joined.clone());
    upsert_env(env, "XDG_DATA_DIRS", joined);
    Ok(())
}

fn search_path(base: &Path, original: Option<&str>) -> Result<String> {
    base.to_str()
        .context("Elvish shell integration path is not valid UTF-8")?;
    let mut paths = vec![base.to_path_buf()];
    if let Some(value) = original.filter(|value| !value.is_empty()) {
        // Keep the user's full path order, including repeats.
        paths.extend(std::env::split_paths(value));
    } else {
        // Elvish treats both unset and explicitly empty XDG_DATA_DIRS as the
        // defaults. Keep that lookup behavior while recording presence exactly.
        paths.extend([
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ]);
    }
    std::env::join_paths(paths)
        .context("Elvish shell integration path cannot be represented in XDG_DATA_DIRS")?
        .into_string()
        .map_err(|_| anyhow::anyhow!("Elvish XDG_DATA_DIRS is not valid UTF-8"))
}
