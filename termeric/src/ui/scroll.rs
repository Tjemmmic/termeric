//! Scrolling a list of equal-height rows to show one of them.

/// The first shown row after scrolling as little as possible from `offset`
/// so that `row` is one of `visible` shown rows. With no visible rows
/// nothing can be revealed and `offset` is kept.
pub fn reveal_row(offset: usize, row: usize, visible: usize) -> usize {
    if row < offset {
        row
    } else if visible > 0 && row >= offset.saturating_add(visible) {
        row + 1 - visible
    } else {
        offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_revealed_with_the_least_scrolling() {
        assert_eq!(reveal_row(5, 2, 4), 2, "above: scroll up to it");
        assert_eq!(reveal_row(5, 7, 4), 5, "shown: stay");
        assert_eq!(reveal_row(5, 8, 4), 5, "last shown row: stay");
        assert_eq!(reveal_row(5, 9, 4), 6, "below: scroll until it is last");
        assert_eq!(reveal_row(5, 9, 0), 5, "nothing visible: stay");
        assert_eq!(reveal_row(usize::MAX, 0, 3), 0);
    }
}
