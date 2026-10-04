//! The desktop clipboard and primary selection. A connection that could not
//! be made is kept with its reason and made again on the next copy or paste
//! a person asks for, so a clipboard manager started late still works.

use termeric_core::ClipboardRequest;

/// One connection to the desktop's clipboards.
pub(super) trait ClipboardBackend {
    fn text(&mut self, selection: ClipboardRequest) -> Result<String, String>;
    fn set_text(&mut self, selection: ClipboardRequest, text: &str) -> Result<(), String>;
}

impl ClipboardBackend for arboard::Clipboard {
    fn text(&mut self, selection: ClipboardRequest) -> Result<String, String> {
        use arboard::GetExtLinux;
        self.get()
            .clipboard(linux_kind(selection))
            .text()
            .map_err(|error| error.to_string())
    }

    fn set_text(&mut self, selection: ClipboardRequest, text: &str) -> Result<(), String> {
        use arboard::SetExtLinux;
        self.set()
            .clipboard(linux_kind(selection))
            .text(text)
            .map_err(|error| error.to_string())
    }
}

fn linux_kind(selection: ClipboardRequest) -> arboard::LinuxClipboardKind {
    match selection {
        ClipboardRequest::Primary => arboard::LinuxClipboardKind::Primary,
        ClipboardRequest::Clipboard => arboard::LinuxClipboardKind::Clipboard,
    }
}

type Connect = Box<dyn FnMut() -> Result<Box<dyn ClipboardBackend>, String>>;

/// Who asked for a clipboard operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClipboardAccess {
    /// A person: copy, paste, middle-click. A failed connection is retried.
    Explicit,
    /// A side effect, such as finishing a selection or a program's OSC 52
    /// write. It uses the connection there is, so it never stalls on one
    /// that keeps failing.
    Implicit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ClipboardError {
    /// There is no connection to the desktop clipboard, for this reason.
    Unavailable(String),
    /// The clipboard refused this read or write.
    Failed(String),
}

impl std::fmt::Display for ClipboardError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(formatter, "clipboard unavailable: {reason}"),
            Self::Failed(reason) => formatter.write_str(reason),
        }
    }
}

pub(super) struct Clipboard {
    backend: Option<Box<dyn ClipboardBackend>>,
    /// Why the last connection attempt failed.
    failure: String,
    connect: Connect,
    /// A program's clipboard write was dropped for want of a connection and
    /// logged; later ones are not, until a connection is made.
    missing_logged: bool,
}

impl Clipboard {
    /// The desktop clipboard, connected now if it can be.
    pub(super) fn system() -> Self {
        Self::connecting(Box::new(|| {
            arboard::Clipboard::new()
                .map(|clipboard| Box::new(clipboard) as Box<dyn ClipboardBackend>)
                .map_err(|error| error.to_string())
        }))
    }

    /// No desktop clipboard at all: a headless app never touches the
    /// person's real one.
    pub(super) fn unavailable() -> Self {
        Self::connecting(Box::new(|| {
            Err("a headless app has no desktop clipboard".to_owned())
        }))
    }

    fn connecting(connect: Connect) -> Self {
        let mut clipboard = Self {
            backend: None,
            failure: String::new(),
            connect,
            missing_logged: false,
        };
        clipboard.connect();
        clipboard
    }

    fn connect(&mut self) {
        match (self.connect)() {
            Ok(backend) => {
                self.backend = Some(backend);
                self.missing_logged = false;
            }
            Err(reason) => {
                log::warn!("Clipboard unavailable: {reason}");
                self.failure = reason;
            }
        }
    }

    fn backend(
        &mut self,
        access: ClipboardAccess,
    ) -> Result<&mut (dyn ClipboardBackend + 'static), ClipboardError> {
        if self.backend.is_none() && access == ClipboardAccess::Explicit {
            self.connect();
        }
        self.backend
            .as_deref_mut()
            .ok_or_else(|| ClipboardError::Unavailable(self.failure.clone()))
    }

    pub(super) fn text(
        &mut self,
        selection: ClipboardRequest,
        access: ClipboardAccess,
    ) -> Result<String, ClipboardError> {
        self.backend(access)?
            .text(selection)
            .map_err(ClipboardError::Failed)
    }

    pub(super) fn set_text(
        &mut self,
        selection: ClipboardRequest,
        text: &str,
        access: ClipboardAccess,
    ) -> Result<(), ClipboardError> {
        self.backend(access)?
            .set_text(selection, text)
            .map_err(ClipboardError::Failed)
    }

    /// Whether this is the first dropped program write since the clipboard
    /// was last reachable, which is the one worth logging.
    pub(super) fn first_missing_write(&mut self) -> bool {
        !std::mem::replace(&mut self.missing_logged, true)
    }
}

/// What tests see of an in-memory clipboard, and how they make it fail.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct MemoryClipboard {
    pub(crate) primary: Option<String>,
    pub(crate) clipboard: Option<String>,
    /// Every read and write fails with this reason while it is set.
    pub(crate) failure: Option<String>,
}

#[cfg(test)]
pub(crate) type SharedMemoryClipboard = std::sync::Arc<std::sync::Mutex<MemoryClipboard>>;

#[cfg(test)]
impl ClipboardBackend for SharedMemoryClipboard {
    fn text(&mut self, selection: ClipboardRequest) -> Result<String, String> {
        let memory = self.lock().unwrap();
        if let Some(reason) = &memory.failure {
            return Err(reason.clone());
        }
        let text = match selection {
            ClipboardRequest::Primary => &memory.primary,
            ClipboardRequest::Clipboard => &memory.clipboard,
        };
        text.clone().ok_or_else(|| "empty".to_owned())
    }

    fn set_text(&mut self, selection: ClipboardRequest, text: &str) -> Result<(), String> {
        let mut memory = self.lock().unwrap();
        if let Some(reason) = &memory.failure {
            return Err(reason.clone());
        }
        let slot = match selection {
            ClipboardRequest::Primary => &mut memory.primary,
            ClipboardRequest::Clipboard => &mut memory.clipboard,
        };
        *slot = Some(text.to_owned());
        Ok(())
    }
}

#[cfg(test)]
impl Clipboard {
    /// A clipboard held in memory, and the handle tests read it through.
    pub(crate) fn in_memory() -> (Self, SharedMemoryClipboard) {
        let memory = SharedMemoryClipboard::default();
        let backend = memory.clone();
        let clipboard = Self::connecting(Box::new(move || {
            Ok(Box::new(backend.clone()) as Box<dyn ClipboardBackend>)
        }));
        (clipboard, memory)
    }

    /// A clipboard whose connection fails with `reason` until `reachable`
    /// is set, and the in-memory clipboard it then connects to.
    pub(crate) fn unreachable_until(
        reason: &str,
        reachable: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> (Self, SharedMemoryClipboard) {
        let memory = SharedMemoryClipboard::default();
        let backend = memory.clone();
        let reason = reason.to_owned();
        let clipboard = Self::connecting(Box::new(move || {
            if reachable.load(std::sync::atomic::Ordering::SeqCst) {
                Ok(Box::new(backend.clone()) as Box<dyn ClipboardBackend>)
            } else {
                Err(reason.clone())
            }
        }));
        (clipboard, memory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn an_explicit_operation_retries_a_failed_connection_and_an_implicit_one_does_not() {
        let reachable = Arc::new(AtomicBool::new(false));
        let (mut clipboard, memory) = Clipboard::unreachable_until("no display", reachable.clone());
        let unavailable = ClipboardError::Unavailable("no display".into());
        assert_eq!(
            clipboard.set_text(ClipboardRequest::Clipboard, "a", ClipboardAccess::Explicit),
            Err(unavailable.clone())
        );
        reachable.store(true, Ordering::SeqCst);
        assert_eq!(
            clipboard.set_text(ClipboardRequest::Primary, "b", ClipboardAccess::Implicit),
            Err(unavailable)
        );
        assert!(clipboard.first_missing_write());
        assert!(!clipboard.first_missing_write());
        assert_eq!(
            clipboard.set_text(ClipboardRequest::Clipboard, "c", ClipboardAccess::Explicit),
            Ok(())
        );
        assert_eq!(memory.lock().unwrap().clipboard.as_deref(), Some("c"));
        assert_eq!(memory.lock().unwrap().primary, None);
        // A new connection logs the next dropped write again.
        assert!(clipboard.first_missing_write());
    }

    #[test]
    fn a_refused_write_is_a_failure_not_an_unavailable_clipboard() {
        let (mut clipboard, memory) = Clipboard::in_memory();
        memory.lock().unwrap().failure = Some("owner went away".into());
        let error = clipboard
            .set_text(ClipboardRequest::Clipboard, "a", ClipboardAccess::Explicit)
            .unwrap_err();
        assert_eq!(error, ClipboardError::Failed("owner went away".into()));
        assert_eq!(error.to_string(), "owner went away");
        assert_eq!(
            ClipboardError::Unavailable("no display".into()).to_string(),
            "clipboard unavailable: no display"
        );
    }
}
