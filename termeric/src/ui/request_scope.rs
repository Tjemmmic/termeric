//! The request a modal card answers. Each time a card is shown it begins a
//! new request, and every action names the request it was meant for, so an
//! action aimed at an earlier showing, or arriving after the card was
//! dismissed, finds nothing to act on.

#[derive(Debug, Clone, Copy, Default)]
pub struct RequestScope {
    id: u64,
    accepting: bool,
}

impl RequestScope {
    /// Begin a new request and return its id. Ids skip 0, which no request
    /// ever has.
    pub fn begin(&mut self) -> u64 {
        self.id = self.id.wrapping_add(1).max(1);
        self.accepting = true;
        self.id
    }

    /// The latest request's id, whether or not it is still awaited.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Whether `id` is the latest request and it still takes actions.
    pub fn is_awaiting(&self, id: u64) -> bool {
        self.accepting && self.id == id
    }

    pub fn is_awaiting_current(&self) -> bool {
        self.is_awaiting(self.id)
    }

    /// Stop taking actions for the latest request. Its card may still be
    /// animating closed, but nothing it shows can act any more.
    pub fn revoke(&mut self) {
        self.accepting = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_latest_unrevoked_request_is_awaited() {
        let mut scope = RequestScope::default();
        assert!(!scope.is_awaiting_current(), "nothing has been shown");
        let first = scope.begin();
        assert_ne!(first, 0);
        assert!(scope.is_awaiting(first));
        let second = scope.begin();
        assert!(!scope.is_awaiting(first));
        assert!(scope.is_awaiting(second));
        scope.revoke();
        assert!(!scope.is_awaiting(second));
        assert_eq!(scope.id(), second);
    }

    #[test]
    fn ids_wrap_past_zero() {
        let mut scope = RequestScope {
            id: u64::MAX,
            accepting: false,
        };
        assert_eq!(scope.begin(), 1);
    }
}
