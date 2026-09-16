//! One in-flight query per picker; the UI retains/coalesces its latest successor.

use super::{RankedCharacter, search_unicode_characters};
use std::sync::mpsc::{self, Receiver, SyncSender};

pub(super) struct Request {
    pub generation: u64,
    pub query: String,
}

pub(super) struct Completion {
    pub generation: u64,
    pub results: Vec<RankedCharacter>,
}

pub(super) struct SearchWorker {
    pub requests: SyncSender<Request>,
    pub completions: Receiver<Completion>,
}

impl SearchWorker {
    pub fn new() -> std::io::Result<Self> {
        Self::with_search(search_unicode_characters)
    }

    pub(super) fn with_search(
        mut search: impl FnMut(&str) -> Vec<RankedCharacter> + Send + 'static,
    ) -> std::io::Result<Self> {
        let (requests, input) = mpsc::sync_channel::<Request>(1);
        let (output, completions) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("unicode-search".into())
            .spawn(move || {
                while let Ok(request) = input.recv() {
                    let results = search(&request.query);
                    if output
                        .send(Completion {
                            generation: request.generation,
                            results,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        // Dropping the channels wakes a parked worker, or lets its current
        // bounded scan finish without joining/blocking the window thread.
        Ok(Self {
            requests,
            completions,
        })
    }
}
