//! Wording shared across chrome text.

/// `count` with the singular or plural noun, as in "1 file" or "3 files".
pub fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_takes_the_singular() {
        assert_eq!(plural(0, "tab", "tabs"), "0 tabs");
        assert_eq!(plural(1, "tab", "tabs"), "1 tab");
        assert_eq!(plural(2, "match", "matches"), "2 matches");
    }
}
