//! Literal branch insertion for a deliberately simple, unquoted command line.

#[derive(Clone, Copy)]
pub(crate) enum BranchQuoting {
    Posix,
    Fish,
}

pub(crate) struct BranchEdit {
    pub line: String,
    pub wire: Vec<u8>,
}

/// This is not a shell parser. Leave quotes, substitutions, operators, escapes,
/// and multiline input to the shell's own completion instead of guessing which
/// syntactic context a repository-supplied name would be inserted into.
pub(super) fn simple_command_character(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.' | '/' | '@' | ':' | '=' | '+')
}

pub(crate) fn branch_edit(
    input: &str,
    branch: &str,
    quoting: Option<BranchQuoting>,
    submit: bool,
) -> Result<BranchEdit, &'static str> {
    let partial = super::detect_git_completion(input)
        .ok_or("Use shell completion for quoted or complex command lines")?;
    if branch.is_empty()
        || branch.len() > 4096
        || branch.starts_with('-')
        || branch.chars().any(|c| {
            c.is_control()
                || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        || !git2::Reference::is_valid_name(&format!("refs/heads/{branch}"))
    {
        return Err("Branch name is not safe for terminal insertion");
    }
    let literal = if branch
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b"_./-".contains(&c))
    {
        branch.to_owned()
    } else {
        match quoting {
            Some(BranchQuoting::Posix) => format!("'{}'", branch.replace('\'', "'\\''")),
            Some(BranchQuoting::Fish) => format!("'{}'", branch.replace('\'', "\\'")),
            None => return Err("Use shell completion: this branch needs a known quoting dialect"),
        }
    };
    let line = format!("{}{literal}", &input[..input.len() - partial.len()]);
    // UTF-8 bytes are not editor characters. Combining/grapheme-aware shell
    // editing is still the shell's responsibility, not a terminal-side model.
    let mut wire = vec![0x7f; partial.chars().count()];
    wire.extend_from_slice(literal.as_bytes());
    if submit {
        wire.push(b'\r');
    }
    Ok(BranchEdit { line, wire })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_and_unicode_edits_preserve_the_prefix_and_use_character_counts() {
        let plain = branch_edit("git switch ma", "main", None, false).unwrap();
        assert_eq!(plain.line, "git switch main");
        assert_eq!(plain.wire, b"\x7f\x7fmain");
        let unicode = branch_edit("git switch é", "été", Some(BranchQuoting::Posix), true).unwrap();
        assert_eq!(unicode.line, "git switch 'été'");
        assert_eq!(unicode.wire, "\x7f'été'\r".as_bytes());
    }

    #[test]
    fn refuses_ambiguous_contexts_invalid_names_and_unknown_quoting() {
        for input in [
            "git switch 'ma",
            "git switch \"ma",
            "git switch $(printf ma",
            "git switch a; ma",
            "git switch a\\ ma",
            "git switch `printf ma",
            "git switch a\nma",
            "git switch a\tma",
        ] {
            assert!(
                super::super::detect_git_completion(input).is_none(),
                "{input:?}"
            );
            assert!(branch_edit(input, "main", Some(BranchQuoting::Posix), true).is_err());
        }
        for branch in [
            "",
            "-flag",
            "a b",
            "a\nb",
            "a\x1bb",
            "a..b",
            "a\\b",
            "a\u{202e}b",
            "a\u{061c}b",
            "a\u{200e}b",
            "a\u{200f}b",
        ] {
            assert!(
                branch_edit("git switch ", branch, Some(BranchQuoting::Posix), false).is_err(),
                "{branch:?}"
            );
        }
        assert!(branch_edit("git switch ", "a;b", None, false).is_err());
        assert!(
            branch_edit(
                "git switch ",
                &"a".repeat(4097),
                Some(BranchQuoting::Posix),
                false
            )
            .is_err()
        );
    }

    #[test]
    fn apostrophes_use_the_selected_dialect() {
        assert_eq!(
            branch_edit("git switch ", "a'b", Some(BranchQuoting::Posix), false)
                .unwrap()
                .line,
            "git switch 'a'\\''b'"
        );
        assert_eq!(
            branch_edit("git switch ", "a'b", Some(BranchQuoting::Fish), false)
                .unwrap()
                .line,
            "git switch 'a\\'b'"
        );
    }

    #[cfg(unix)]
    #[test]
    fn valid_metacharacter_refs_round_trip_as_one_literal_posix_argument() {
        let directory = tempfile::tempdir().unwrap();
        for branch in [
            "topic;printf${IFS}UNEXPECTED",
            "topic$(printf${IFS}UNEXPECTED)",
            "topic`printf${IFS}UNEXPECTED`",
            "a'b\"c",
            "topic&other",
            "topic|other",
            "#topic",
            "topic>file",
            "été",
            "topic!history",
            "a{b,c}",
        ] {
            // libgit2 validation in branch_edit establishes these are real ref
            // names, not merely arbitrary adversarial strings.
            let edit =
                branch_edit("git switch ", branch, Some(BranchQuoting::Posix), false).unwrap();
            let literal = edit.line.strip_prefix("git switch ").unwrap();
            let output = std::process::Command::new("/bin/sh")
                .args(["-c", &format!("printf '%s\\0' {literal}")])
                .current_dir(directory.path())
                .env_clear()
                .output()
                .unwrap();
            assert!(output.status.success(), "{branch:?}: {:?}", output.stderr);
            assert_eq!(
                output.stdout,
                [branch.as_bytes(), b"\0"].concat(),
                "{branch:?}"
            );
        }
    }
}
