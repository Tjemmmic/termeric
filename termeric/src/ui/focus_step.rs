//! Moving keyboard focus through a panel's controls in order (the Agents and
//! Commands panels).

/// One step of keyboard focus through a list of controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusStep {
    Next,
    Previous,
    First,
    Last,
}

impl FocusStep {
    /// The index focus moves to among `count` controls from `current`,
    /// wrapping at either end. With nothing focused, Next starts at the first
    /// control and Previous at the last. `count` must not be zero.
    pub fn target(self, current: Option<usize>, count: usize) -> usize {
        let last = count - 1;
        match (self, current) {
            (Self::First, _) | (Self::Next, None) => 0,
            (Self::Last, _) | (Self::Previous, None) => last,
            (Self::Next, Some(index)) => (index + 1) % count,
            (Self::Previous, Some(index)) => (index + last) % count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_wrap_at_both_ends_and_start_from_the_matching_edge() {
        assert_eq!(FocusStep::Next.target(None, 4), 0);
        assert_eq!(FocusStep::Previous.target(None, 4), 3);
        assert_eq!(FocusStep::Next.target(Some(1), 4), 2);
        assert_eq!(FocusStep::Next.target(Some(3), 4), 0);
        assert_eq!(FocusStep::Previous.target(Some(0), 4), 3);
        assert_eq!(FocusStep::Previous.target(Some(2), 4), 1);
        assert_eq!(FocusStep::First.target(Some(2), 4), 0);
        assert_eq!(FocusStep::Last.target(Some(0), 4), 3);
        assert_eq!(FocusStep::Next.target(Some(0), 1), 0);
    }
}
