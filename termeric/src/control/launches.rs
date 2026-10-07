//! Bounded completion records keyed by launch identity, independent of UI paths.

use super::protocol::{LaunchState, valid_request_id};
use std::collections::VecDeque;
use termeric_core::pty::ChildObserver;

pub(crate) const MAX_LAUNCH_RECORDS: usize = 256;

struct Entry {
    id: String,
    child: ChildObserver,
}

#[derive(Default)]
pub(crate) struct LaunchLedger {
    entries: VecDeque<Entry>,
}

impl LaunchLedger {
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.entries.iter().any(|entry| entry.id == id)
    }

    /// Admission runs before any spawn. A completed record may be forgotten;
    /// running children and observation failures are never eviction candidates.
    pub(crate) fn prepare(&mut self, id: &str) -> Result<(), String> {
        if !valid_request_id(id) || self.contains(id) {
            return Err(
                "Launch request ID is invalid or already retained; use a new request ID".into(),
            );
        }
        if self.entries.len() >= MAX_LAUNCH_RECORDS {
            let completed = self
                .entries
                .iter()
                .position(|entry| entry.child.poll().is_ok_and(|result| result.exit.is_some()));
            let Some(index) = completed else {
                return Err(
                    "GUI launch tracking limit reached; wait for a retained child to exit".into(),
                );
            };
            self.entries.remove(index);
        }
        Ok(())
    }

    /// Called synchronously after successful creation, before returning to the
    /// event loop. The preceding admission reserves capacity without a waiter.
    pub(crate) fn record(&mut self, id: &str, child: ChildObserver) {
        assert!(self.entries.len() < MAX_LAUNCH_RECORDS && !self.contains(id));
        self.entries.push_back(Entry {
            id: id.to_owned(),
            child,
        });
    }

    pub(crate) fn query(&self, id: &str) -> Result<(LaunchState, bool), String> {
        let child = &self
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .ok_or("GUI launch result is unknown or expired")?
            .child;
        let observation = child
            .poll()
            .map_err(|_| "Could not observe GUI launch child status")?;
        let state = observation
            .exit
            .map_or(LaunchState::Running {}, |exit| LaunchState::Exited {
                exit_code: exit.exit_code,
                success: exit.success,
            });
        Ok((state, observation.terminal_closed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termeric_core::Pty;
    use termeric_core::pty::{PtyCommand, default_pty_size};

    #[test]
    fn ledger_bounds_live_records_then_evicts_only_oldest_completion() {
        let pty = Pty::spawn(
            default_pty_size(80, 24),
            PtyCommand {
                program: Some("/bin/sh"),
                args: &["-c".into(), "read value; exit 9".into()],
                ..PtyCommand::default()
            },
        )
        .unwrap();
        let (mut handle, reader) = pty.take_reader();
        let observer = handle.child_observer();
        let mut ledger = LaunchLedger::default();
        // Duplicate observers exercise the record budget without spawning 256
        // processes. Each record still has a unique launch identity.
        for index in 0..MAX_LAUNCH_RECORDS {
            let id = format!("{index:032x}");
            ledger.prepare(&id).unwrap();
            ledger.record(&id, observer.clone());
        }
        assert!(
            ledger
                .prepare(&"f".repeat(32))
                .unwrap_err()
                .contains("limit")
        );
        assert!(ledger.prepare(&"0".repeat(32)).is_err());
        assert!(ledger.query(&"e".repeat(32)).is_err());
        assert_eq!(
            ledger.query(&"0".repeat(32)).unwrap(),
            (LaunchState::Running {}, false)
        );
        handle.write_all(b"\n").unwrap();
        assert!(crate::test_support::poll_until(
            std::time::Duration::from_secs(2),
            || observer.poll().unwrap().exit.is_some()
        ));
        ledger.prepare(&"f".repeat(32)).unwrap();
        ledger.record(&"f".repeat(32), observer);
        assert_eq!(ledger.entries.len(), MAX_LAUNCH_RECORDS);
        assert!(!ledger.contains(&"0".repeat(32)));
        assert!(ledger.contains(&format!("{:032x}", 1)));
        drop(handle);
        drop(reader);
        assert_eq!(
            ledger.query(&"f".repeat(32)).unwrap(),
            (
                LaunchState::Exited {
                    exit_code: 9,
                    success: false
                },
                true
            )
        );
        assert!(ledger.prepare("invalid").is_err());
    }
}
