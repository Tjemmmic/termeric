//! Cross-module runtime fixtures. These deliberately exercise real public
//! boundaries instead of replacing terminal, tab, or plugin behavior with mocks.

use crate::config::ShellConfig;
use crate::harvest::{HarvestMode, HarvestSource, HarvestTerminalState};
use crate::session::transcript::{EntryKind, TranscriptEntry};
use crate::tabs::{TabManager, TabType};
use crate::ui::geometry::Bounds;
use serde::Serialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use termeric_core::{CellFlags, Terminal};

mod benchmarks;
mod soak;

/// An `sh -c` script that shows what input reached its terminal: with no
/// echo or line editing it prints `READY`, then the first `count` bytes it
/// reads, in hex, and stays a moment so its output is read before it exits.
/// Write to it only once it shows `READY`.
pub(crate) fn byte_probe(count: usize) -> String {
    format!(
        "stty raw -echo; printf READY; dd bs=1 count={count} 2>/dev/null | od -An -tx1 -v; sleep 0.2"
    )
}

/// [`byte_probe`] for one byte, which shows whether input reached its
/// terminal at all. `App::assert_pty_untouched` writes the byte it expects
/// first.
pub(crate) const FIRST_BYTE_PROBE: &str =
    "stty raw -echo; printf READY; dd bs=1 count=1 2>/dev/null | od -An -tx1 -v; sleep 0.2";

/// `sh -c` arguments for a child that stays alive through a test without
/// printing anything: a real PTY whose process is still running when the
/// test looks.
pub(crate) fn sleeping_args() -> Vec<String> {
    vec!["-c".into(), "sleep 2".into()]
}

/// Point the catalog's agent `id` at the fake agent `script` in
/// `tests/fixtures/agent_view`, answering at once: `delay_var` is the
/// environment variable that fake waits by. The fakes are Python scripts;
/// CI has python3, so its absence fails the test rather than skipping it.
pub(crate) fn fake_agent(
    config: &mut crate::config::Config,
    id: &str,
    script: &str,
    delay_var: &str,
) {
    assert!(
        crate::agents::find_executable("python3").is_some(),
        "python3 is required by this test"
    );
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/agent_view")
        .join(script);
    config.agents.insert(
        id.into(),
        crate::agents::AgentConfig {
            program: Some(script.display().to_string()),
            env: [(delay_var.to_owned(), "0".to_owned())].into(),
            ..Default::default()
        },
    );
}

/// `/bin/sh` running [`sleeping_args`].
pub(crate) fn sleeping_shell() -> ShellConfig {
    ShellConfig {
        program: Some("/bin/sh".into()),
        args: sleeping_args(),
        env: Default::default(),
    }
}

/// A plugin with nothing but an identity, so the manager accepts results
/// addressed to it.
pub(crate) fn identity_plugin(
    id: &'static str,
    name: &'static str,
) -> Box<dyn crate::plugin::hooks::Plugin> {
    #[derive(Debug)]
    struct IdentityPlugin {
        id: &'static str,
        name: &'static str,
    }

    impl crate::plugin::hooks::Plugin for IdentityPlugin {
        fn id(&self) -> &str {
            self.id
        }

        fn name(&self) -> &str {
            self.name
        }
    }

    Box::new(IdentityPlugin { id, name })
}

/// A plugin that counts the hooks `counted` accepts into `seen`, for tests
/// that check a hook fires exactly as often as it should.
pub(crate) fn hook_counter(
    counted: fn(&crate::plugin::Hook) -> bool,
    seen: &std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> Box<dyn crate::plugin::hooks::Plugin> {
    #[derive(Debug)]
    struct HookCounter {
        counted: fn(&crate::plugin::Hook) -> bool,
        seen: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl crate::plugin::hooks::Plugin for HookCounter {
        fn id(&self) -> &str {
            "hook-counter"
        }

        fn name(&self) -> &str {
            "Hook Counter"
        }

        fn on_hook(
            &mut self,
            hook: &crate::plugin::Hook,
            _ctx: &crate::plugin::PluginContext,
        ) -> Option<crate::plugin::PluginAction> {
            if (self.counted)(hook) {
                self.seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            None
        }
    }

    Box::new(HookCounter {
        counted,
        seen: std::sync::Arc::clone(seen),
    })
}

/// Whether a process with `process_id` exists; signal 0 only checks.
pub(crate) fn process_is_alive(process_id: libc::pid_t) -> bool {
    // SAFETY: signal 0 sends nothing.
    unsafe { libc::kill(process_id, 0) == 0 }
}

/// Whether `process_id` is gone within a few seconds; whoever adopted it
/// may take a moment to reap it.
pub(crate) fn process_ends(process_id: libc::pid_t) -> bool {
    poll_until(Duration::from_secs(5), || !process_is_alive(process_id))
}

/// Shrinks `socket`'s send buffer to about `bytes` (the kernel doubles the
/// request), so a test can fill it, and block a writer, with little data.
pub(crate) fn shrink_send_buffer(socket: &impl std::os::fd::AsRawFd, bytes: libc::c_int) {
    // SAFETY: the descriptor is live and the option points to one c_int for
    // exactly its size; only this socket's buffer changes.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            (&bytes as *const libc::c_int).cast(),
            std::mem::size_of_val(&bytes) as libc::socklen_t,
        )
    };
    assert_eq!(result, 0, "{}", std::io::Error::last_os_error());
}

/// How many bytes wait unread on `socket`, without reading any.
pub(crate) fn unread_bytes(socket: &impl std::os::fd::AsRawFd) -> usize {
    let mut available: libc::c_int = 0;
    // SAFETY: FIONREAD writes one c_int and neither reads nor drains bytes.
    let result = unsafe { libc::ioctl(socket.as_raw_fd(), libc::FIONREAD, &mut available) };
    assert_eq!(result, 0, "{}", std::io::Error::last_os_error());
    usize::try_from(available).expect("FIONREAD reports a count")
}

/// Every entry of the finished transcript at `path`, one JSON line each.
pub(crate) fn transcript_entries(path: &std::path::Path) -> Vec<TranscriptEntry> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The contents of `entries` of `kind`, joined in order.
pub(crate) fn transcript_text(entries: &[TranscriptEntry], kind: EntryKind) -> String {
    entries
        .iter()
        .filter(|entry| entry.kind == kind)
        .map(|entry| entry.content.as_str())
        .collect()
}

/// Run `step` until it returns true or `timeout` passes, pausing briefly
/// between tries. Returns whether it succeeded.
pub(crate) fn poll_until(timeout: Duration, mut step: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if step() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub(crate) fn seed_pixel_geometry_retention_probe(terminal: &mut Terminal) {
    assert_eq!((terminal.grid().cols(), terminal.grid().rows()), (8, 5));
    terminal.set_scrollback_limits(0, 0);
    terminal.process(b"\x1bPq\"1;1!4~\x1b\\\x1b[5;1HX");
    let image = terminal.graphics_placements().next().unwrap();
    assert_eq!((image.width, image.height), (Some(2), Some(2)));
    terminal.process(b"\x1b[?2026h");
    terminal.presentation_grid_mut().set_scrollback_limit(0);
}

pub(crate) fn assert_pixel_geometry_retention_probe(terminal: &Terminal) {
    assert_eq!((terminal.grid().cols(), terminal.grid().rows()), (8, 2));
    assert_eq!(terminal.grid().scrollback_len(), 0);
    let live = terminal.graphics_placements().next().unwrap();
    let frozen = terminal.presentation_images();
    assert_eq!(frozen.len(), 1);
    for image in [live, &frozen[0].placement] {
        assert_eq!(
            (image.row, image.width, image.height),
            (-3, Some(4), Some(6))
        );
        assert_eq!((image.pixel_width, image.pixel_height), (Some(4), Some(6)));
    }
    assert!(std::sync::Arc::ptr_eq(
        &terminal.get_image(live.image_id).unwrap().data,
        &frozen[0].image.data
    ));
}

struct TerminalFixture {
    terminal: Terminal,
}

impl TerminalFixture {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            terminal: Terminal::new(cols, rows),
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> &mut Self {
        self.terminal.process(bytes);
        self
    }
}

#[test]
fn ansi_hyperlink_and_semantic_marks_cross_the_parser_grid_and_harvest() {
    let mut fixture = TerminalFixture::new(80, 24);
    fixture.feed(b"\x1b[31mred\x1b[0m \x1b]8;;https://example.com\x07link\x1b]8;;\x07\r\n");
    let link_cell = fixture.terminal.grid().cell(0, 4).unwrap();
    assert!(link_cell.flags.contains(CellFlags::HYPERLINK));
    let link_id = link_cell.hyperlink_id.unwrap();
    assert_eq!(
        fixture.terminal.get_hyperlink_url(link_id),
        Some("https://example.com")
    );

    fixture.feed(b"\x1b]133;A\x07\x1b]133;B\x07echo ok\r\n\x1b]133;C\x07ok\r\n\x1b]133;D;0\x07");
    let mut terminal_state = HarvestTerminalState::default();
    let regions = terminal_state.observe(fixture.terminal.prompt_tracker(), true);
    assert_eq!(regions.len(), 1);
    let mut harvest = HarvestMode::with_capacity(10);
    harvest.record_command(
        HarvestSource {
            tab_id: 1,
            pane_id: 0,
        },
        &regions[0],
        "echo ok",
        false,
        "ok",
        false,
    );
    assert_eq!(harvest.cellar.len(), 1);
    assert_eq!(harvest.cellar.latest().unwrap().command, "echo ok");
    assert_eq!(harvest.cellar.latest().unwrap().output, "ok");
    assert_eq!(harvest.cellar.latest().unwrap().exit_code, Some(0));
}

#[test]
fn resize_fixture_reflows_real_terminal_content() {
    let mut fixture = TerminalFixture::new(12, 4);
    fixture.feed(b"one two three four");
    fixture.terminal.resize(7, 4);
    assert!(fixture.terminal.rows_text(0, 3).contains("one"));
    assert!(fixture.terminal.rows_text(0, 3).contains("four"));
    fixture.terminal.resize(20, 4);
    assert!(
        fixture
            .terminal
            .rows_text(0, 3)
            .contains("one two three four")
    );
}

#[test]
fn plugin_quick_exit_crosses_pty_tab_and_diagnostic_capture() {
    let mut tabs = TabManager::new(
        80,
        24,
        ShellConfig::default(),
        100,
        termeric_core::DEFAULT_SCROLLBACK_MEMORY_BYTES,
    );
    tabs.new_tab_with_command(
        "/bin/sh",
        &["-c".into(), "printf 'plugin failed\\n'; exit 7".into()],
        None,
        Some("fixture"),
        Some(TabType::Plugin {
            plugin_id: "fixture-plugin".into(),
        }),
        HashMap::new(),
    )
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    let captured = loop {
        tabs.poll_events();
        let captured = tabs.drain_quick_exit_plugin_info();
        if !captured.is_empty() || Instant::now() >= deadline {
            break captured;
        }
        std::thread::yield_now();
    };
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].0, "fixture-plugin");
    assert!(captured[0].1.join("\n").contains("plugin failed"));
}

#[test]
fn the_first_byte_probe_is_the_one_byte_probe() {
    assert_eq!(FIRST_BYTE_PROBE, byte_probe(1));
}
