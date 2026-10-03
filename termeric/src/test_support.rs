//! Cross-module runtime fixtures. These deliberately exercise real public
//! boundaries instead of replacing terminal, tab, or plugin behavior with mocks.

use crate::config::ShellConfig;
use crate::harvest::{HarvestMode, HarvestSource, HarvestTerminalState};
use crate::tabs::{TabManager, TabType};
use crate::ui::geometry::Bounds;
use serde::Serialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use termeric_core::{CellFlags, Terminal};

#[cfg(unix)]
pub(crate) fn seed_pixel_geometry_retention_probe(terminal: &mut Terminal) {
    assert_eq!((terminal.grid().cols(), terminal.grid().rows()), (8, 5));
    terminal.set_scrollback_limits(0, 0);
    terminal.process(b"\x1bPq\"1;1!4~\x1b\\\x1b[5;1HX");
    let image = terminal.graphics_placements().next().unwrap();
    assert_eq!((image.width, image.height), (Some(2), Some(2)));
    terminal.process(b"\x1b[?2026h");
    terminal.presentation_grid_mut().set_scrollback_limit(0);
}

#[cfg(unix)]
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

#[derive(Serialize)]
struct BenchmarkMetric<'a> {
    name: &'a str,
    unit: &'a str,
    samples: Vec<f64>,
    workload: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<&'a str>,
}

fn emit_benchmark(
    name: &'static str,
    unit: &'static str,
    samples: Vec<f64>,
    workload: &'static str,
    context: Option<&str>,
) {
    println!(
        "TERMERIC_BENCH {}",
        serde_json::to_string(&BenchmarkMetric {
            name,
            unit,
            samples,
            workload,
            context,
        })
        .unwrap()
    );
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
    let regions =
        terminal_state.observe(&fixture.terminal.shell_integration().prompt_tracker, true);
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
fn plugin_quick_exit_crosses_pty_tab_and_diagnostic_capture_when_shell_exists() {
    if std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("exit 0")
        .status()
        .is_err()
    {
        eprintln!("skipping plugin quick-exit fixture: /bin/sh is unavailable");
        return;
    }

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
#[ignore = "needs a GPU or Mesa's software Vulkan: cargo test -p termeric -- --ignored gpu_render_smoke"]
fn gpu_render_smoke() {
    let instance_descriptor = wgpu::InstanceDescriptor::from_env_or_default();
    let enabled_backends = instance_descriptor.backends;
    let instance = wgpu::Instance::new(&instance_descriptor);
    let require_hardware = std::env::var("TERMERIC_GPU_SMOKE_HARDWARE").as_deref() == Ok("1");
    let adapter = crate::renderer::discover_adapter(
        &instance,
        enabled_backends,
        None,
        wgpu::PowerPreference::LowPower,
        !require_hardware,
    )
    .unwrap_or_else(|| {
        let available = instance
            .enumerate_adapters(enabled_backends)
            .into_iter()
            .map(|adapter| {
                let info = adapter.get_info();
                format!("{:?}/{:?}/{:?}", info.name, info.backend, info.device_type)
            })
            .collect::<Vec<_>>();
        if require_hardware {
            panic!(
                "TERMERIC_GPU_SMOKE_HARDWARE=1 but no headless hardware adapter was found; \
                 enabled backends={enabled_backends:?}; enumerated adapters={available:?}"
            );
        }
        panic!(
            "no WGPU software fallback adapter was found; \
             enabled backends={enabled_backends:?}; enumerated adapters={available:?}. \
             Install a WGPU-compatible CPU implementation such as Mesa llvmpipe/lavapipe \
             (mesa-vulkan-drivers on Debian/Ubuntu)"
        );
    });
    let adapter_info = adapter.get_info();
    if require_hardware {
        assert_ne!(
            adapter_info.device_type,
            wgpu::DeviceType::Cpu,
            "hardware smoke selected a CPU adapter: {adapter_info:?}"
        );
    } else {
        assert_eq!(
            adapter_info.device_type,
            wgpu::DeviceType::Cpu,
            "software smoke selected a non-CPU adapter: {adapter_info:?}"
        );
    }
    eprintln!("GPU render smoke adapter: {adapter_info:?}");
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::default(),
            label: Some("Termeric headless smoke device"),
        },
        None,
    ))
    .expect("headless GPU device request failed");
    crate::renderer::headless_unicode_render_smoke(&device, &queue)
        .expect("Unicode grapheme render smoke failed");
    crate::renderer::headless_ui_text_smoke(&device, &queue).expect("UI text smoke failed");
    crate::renderer::headless_color_emoji_render_smoke(&device, &queue)
        .expect("color emoji render smoke failed");
    crate::renderer::headless_glyph_atlas_stress_smoke(&device, &queue)
        .expect("paged glyph atlas stress smoke failed");
    crate::renderer::headless_sgr_render_smoke(&device, &queue)
        .expect("SGR attribute render smoke failed");
    crate::renderer::headless_ligature_render_smoke(&device, &queue)
        .expect("ligature render smoke failed");
    crate::renderer::headless_palette_input_smoke(&device, &queue)
        .expect("palette input geometry smoke failed");
    crate::renderer::headless_image_render_smoke(&device, &queue)
        .expect("Kitty graphics render smoke failed");
    crate::renderer::headless_theme_color_fidelity_smoke(&device, &queue)
        .expect("theme color fidelity smoke failed");
    crate::renderer::headless_background_render_smoke(&device, &queue)
        .expect("background image render smoke failed");
    let (pixels, width, height) = crate::renderer::headless_gui_visual_snapshot(&device, &queue)
        .expect("GUI visual baseline render failed");
    if let Some(output) = std::env::var_os("TERMERIC_GUI_VISUAL_OUTPUT") {
        let output = std::path::PathBuf::from(output);
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).expect("create GUI visual artifact directory");
        }
        image::save_buffer(&output, &pixels, width, height, image::ColorType::Rgba8)
            .expect("save GUI visual artifact");
    }
    let signature = VisualSignature::of(&pixels, width, height);
    let baseline = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/renderer/visual_baseline.json"
    );
    if std::env::var("TERMERIC_GUI_VISUAL_UPDATE").as_deref() == Ok("1") {
        let json = serde_json::to_string_pretty(&signature).unwrap();
        std::fs::write(baseline, json + "\n").expect("write the visual baseline");
    }
    let expected: VisualSignature =
        serde_json::from_str(&std::fs::read_to_string(baseline).unwrap()).unwrap();
    let differences = signature.differences(&expected);
    assert!(
        differences.is_empty(),
        "GUI visual regression: {differences:?}; if the change is intended, rerun with \
         TERMERIC_GUI_VISUAL_UPDATE=1"
    );
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Termeric headless smoke target"),
        size: wgpu::Extent3d {
            width: 16,
            height: 16,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Termeric headless smoke encoder"),
    });
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Termeric headless smoke pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.1,
                        g: 0.07,
                        b: 0.04,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
    }
    queue.submit([encoder.finish()]);
}

/// Where ink falls in a rendered frame, coarse enough to ignore
/// antialiasing: the frame is shrunk to 128×32 and compared by how much is
/// inked, its bounds, its mean color, and its row and column profiles.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct VisualSignature {
    normalized_size: [u32; 2],
    ink_pixels: usize,
    ink_bbox: [u32; 4],
    mean_ink_rgb: [f64; 3],
    row_ink: Vec<u32>,
    column_ink: Vec<u32>,
}

impl VisualSignature {
    const SIZE: [u32; 2] = [128, 32];

    fn of(rgba: &[u8], width: u32, height: u32) -> Self {
        let image = image::RgbaImage::from_raw(width, height, rgba.to_vec())
            .expect("frame pixels match its size");
        let [columns, rows] = Self::SIZE;
        let small =
            image::imageops::resize(&image, columns, rows, image::imageops::FilterType::Triangle);
        let mut signature = Self {
            normalized_size: Self::SIZE,
            ink_pixels: 0,
            ink_bbox: [u32::MAX, u32::MAX, 0, 0],
            mean_ink_rgb: [0.0; 3],
            row_ink: vec![0; rows as usize],
            column_ink: vec![0; columns as usize],
        };
        for (x, y, pixel) in small.enumerate_pixels() {
            let [red, green, blue, _] = pixel.0;
            if red.max(green).max(blue) <= 8 {
                continue;
            }
            signature.ink_pixels += 1;
            let [left, top, right, bottom] = &mut signature.ink_bbox;
            (*left, *top, *right, *bottom) = (
                (*left).min(x),
                (*top).min(y),
                (*right).max(x),
                (*bottom).max(y),
            );
            for (sum, channel) in signature.mean_ink_rgb.iter_mut().zip([red, green, blue]) {
                *sum += f64::from(channel);
            }
            signature.row_ink[y as usize] += 1;
            signature.column_ink[x as usize] += 1;
        }
        assert!(signature.ink_pixels > 0, "the frame has no visible pixels");
        for sum in &mut signature.mean_ink_rgb {
            *sum /= signature.ink_pixels as f64;
        }
        signature
    }

    fn differences(&self, expected: &Self) -> Vec<String> {
        let mut differences = Vec::new();
        let ink_change = self.ink_pixels.abs_diff(expected.ink_pixels) as f64
            / expected.ink_pixels.max(1) as f64;
        if ink_change > 0.12 {
            differences.push(format!(
                "inked pixels {} vs {}",
                self.ink_pixels, expected.ink_pixels
            ));
        }
        if self
            .ink_bbox
            .iter()
            .zip(expected.ink_bbox)
            .any(|(actual, expected)| actual.abs_diff(expected) > 4)
        {
            differences.push(format!(
                "ink bounds {:?} vs {:?}",
                self.ink_bbox, expected.ink_bbox
            ));
        }
        if self
            .mean_ink_rgb
            .iter()
            .zip(expected.mean_ink_rgb)
            .any(|(actual, expected)| (actual - expected).abs() > 20.0)
        {
            differences.push(format!(
                "mean ink color {:?} vs {:?}",
                self.mean_ink_rgb, expected.mean_ink_rgb
            ));
        }
        let profile_distance = |actual: &[u32], expected: &[u32]| {
            let total: u32 = actual
                .iter()
                .zip(expected)
                .map(|(a, e)| a.abs_diff(*e))
                .sum();
            f64::from(total) / f64::from(expected.iter().sum::<u32>().max(1))
        };
        for (name, actual, expected) in [
            ("row", &self.row_ink, &expected.row_ink),
            ("column", &self.column_ink, &expected.column_ink),
        ] {
            let distance = profile_distance(actual, expected);
            if distance > 0.18 {
                differences.push(format!("{name} ink profile moved by {distance:.3}"));
            }
        }
        differences
    }
}

#[cfg(debug_assertions)]
fn require_release_profile() {
    panic!("performance suite requires --release");
}

#[cfg(not(debug_assertions))]
fn require_release_profile() {}

/// Opt-in process-level resource exercise: repeats terminal, pane, tab,
/// session, transcript, and diff work, then checks this process's own memory,
/// threads, open files, and children stayed within bounds.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "a resource soak: cargo test -p termeric -- --ignored scheduled_resource_soak_scenario"]
fn scheduled_resource_soak_scenario() {
    use crate::panes::{PaneManager, SplitDirection};
    use crate::session::Session;
    use crate::session::save::save_session_to_dir;
    use crate::session::transcript::{TranscriptRecorder, TranscriptWrite};
    use crate::terminal::search::Search;
    use std::fmt::Write as _;
    use termeric_core::graphics::ImageStorage;
    use termeric_core::{ImageId, StoredImage};

    let duration = std::env::var("TERMERIC_SOAK_DURATION_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(5)
        .max(1);
    let deadline = Instant::now() + Duration::from_secs(duration);
    let workspace = tempfile::tempdir().unwrap();
    let session_dir = workspace.path().join("session");
    let transcript_dir = workspace.path().join("transcripts");

    let mut terminal = Terminal::new(120, 40);
    terminal.set_scrollback_limits(50_000, 16 * 1024 * 1024);
    let mut images = ImageStorage::new(32, 4 * 1024 * 1024);
    let mut diffs = crate::terminal::diff_detector::DiffDetector::new();
    let mut iteration = 0usize;
    let mut baseline = None;
    let mut most_children = 0;
    let mut largest_rss_kib = 0;

    while Instant::now() < deadline {
        let mut output = String::with_capacity(32 * 1024);
        for line in 0..128 {
            writeln!(
                output,
                "{iteration:06}:{line:03} needle ASCII 界 e\u{301} 👩\u{200d}💻 \x1b[38;2;120;180;90mstyled\x1b[0m\r"
            )
            .unwrap();
        }
        terminal.process(output.as_bytes());
        terminal.resize(if iteration.is_multiple_of(2) { 80 } else { 132 }, 40);
        let mut search = Search::new();
        search.start();
        for character in "needle".chars() {
            search.push_char(character, terminal.grid());
        }
        search.finish(terminal.grid());
        assert!(search.match_count() > 0);

        let pixels = vec![(iteration % 251) as u8; 64 * 64 * 4];
        images
            .store(StoredImage {
                id: ImageId(iteration as u32),
                width: 64,
                height: 64,
                data: pixels.into(),
                revision: iteration as u64,
            })
            .unwrap();
        assert!(images.memory_usage() <= 4 * 1024 * 1024);
        assert!(images.len() <= 32);

        let shell = ShellConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "printf 'pane-soak\\n'".into()],
            env: Default::default(),
        };
        let mut panes = PaneManager::new(
            100,
            36,
            shell,
            1_000,
            8 * 1024 * 1024,
            1_200.0,
            720.0,
            10.0,
            20.0,
        )
        .unwrap();
        panes.split(SplitDirection::Vertical).unwrap();
        panes.split(SplitDirection::Horizontal).unwrap();
        panes.resize(900.0, 600.0, 9.0, 18.0);
        let pane_deadline = Instant::now() + Duration::from_secs(2);
        while !panes.all_closed() && Instant::now() < pane_deadline {
            panes.poll_events(|_, _| {});
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(panes.all_closed(), "pane child did not exit during soak");
        drop(panes);

        let mut tabs = TabManager::new(100, 36, ShellConfig::default(), 1_000, 8 * 1024 * 1024);
        for tab in 0..4 {
            tabs.new_tab_with_command(
                "/bin/sh",
                &["-c".into(), format!("printf 'tab-{tab}-soak\\n'")],
                None,
                Some("soak"),
                None,
                HashMap::new(),
            )
            .unwrap();
        }
        let tab_deadline = Instant::now() + Duration::from_secs(2);
        while !tabs.is_empty() && Instant::now() < tab_deadline {
            tabs.poll_events();
            tabs.cleanup_closed(false, false);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(tabs.is_empty(), "tab child did not exit during soak");

        let mut session = Session::new("soak".into(), Some("/tmp".into()));
        session.id = "resource-soak".into();
        session.updated_at = format!("iteration-{iteration}");
        save_session_to_dir(&session, &session_dir).unwrap();

        let mut transcript =
            TranscriptRecorder::new(&transcript_dir, Some("soak"), Some("main"), "local", "soak")
                .unwrap();
        let mut accepted_entries = 0usize;
        let mut dropped_entries = 0usize;
        for entry in 0..64 {
            let output_result = transcript
                .record_output(format!("iteration={iteration} output={entry} 界\n"))
                .unwrap();
            if output_result == TranscriptWrite::Written {
                accepted_entries += 1;
            } else {
                dropped_entries += 1;
            }
            if entry % 16 == 0 {
                let input_result = transcript.record_input("echo soak\r").unwrap();
                if input_result == TranscriptWrite::Written {
                    accepted_entries += 1;
                } else {
                    dropped_entries += 1;
                }
            }
        }
        // Every entry is either written or counted as dropped.
        assert_eq!(accepted_entries + dropped_entries, 68);
        let queued_bytes = transcript.queued_bytes_for_diagnostics();
        let transcript_path = transcript.path().to_path_buf();
        transcript.finish(Duration::from_secs(2)).unwrap();
        let drained_queue_bytes = transcript.queued_bytes_for_diagnostics();
        assert_eq!(drained_queue_bytes, 0, "transcript queue did not drain");
        std::fs::remove_file(transcript_path).unwrap();

        soak_diff_history(&mut diffs, iteration);
        assert!(
            queued_bytes <= 2 * 1024 * 1024,
            "transcript queue grew to {queued_bytes} bytes"
        );

        iteration += 1;
        let sample = ProcessResources::sample();
        most_children = most_children.max(sample.children);
        largest_rss_kib = largest_rss_kib.max(sample.rss_kib);
        // The first iteration warms caches, pools, and worker threads up.
        baseline.get_or_insert(sample);
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(iteration > 1, "the soak needs at least two iterations");
    assert!(terminal.memory_usage().scrollback_bytes <= 16 * 1024 * 1024);
    let first = baseline.expect("a sample after the first iteration");
    let last = ProcessResources::sample();
    assert!(
        largest_rss_kib <= 512 * 1024,
        "RSS peaked at {largest_rss_kib} KiB"
    );
    assert!(
        last.rss_kib.saturating_sub(first.rss_kib) <= 128 * 1024,
        "RSS grew from {} to {} KiB",
        first.rss_kib,
        last.rss_kib
    );
    assert!(
        last.threads <= first.threads + 4,
        "threads grew {first:?} -> {last:?}"
    );
    assert!(
        last.fds <= first.fds + 8,
        "file descriptors grew {first:?} -> {last:?}"
    );
    assert!(
        most_children <= 16,
        "{most_children} child processes at once"
    );
}

/// This process's resident memory, threads, open files, and live children.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug)]
struct ProcessResources {
    rss_kib: u64,
    threads: usize,
    fds: usize,
    children: usize,
}

#[cfg(target_os = "linux")]
impl ProcessResources {
    fn sample() -> Self {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let rss_kib = status
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))
            .and_then(|value| value.split_whitespace().next()?.parse().ok())
            .expect("VmRSS in /proc/self/status");
        let count = |path: &str| std::fs::read_dir(path).unwrap().count();
        let children = std::fs::read_dir("/proc/self/task")
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
            .map(|children| children.split_whitespace().count())
            .sum();
        Self {
            rss_kib,
            threads: count("/proc/self/task"),
            fds: count("/proc/self/fd"),
            children,
        }
    }
}

/// Feed one persistent history past both its count and owned-capacity ceilings.
/// All bytes are synthetic; no provider, process, or working tree is involved.
#[cfg(target_os = "linux")]
fn soak_diff_history(
    detector: &mut crate::terminal::diff_detector::DiffDetector,
    iteration: usize,
) -> (usize, usize, usize, usize, usize) {
    let before = detector.history_diagnostics().0;
    let mut completed = 0;
    let mut peak_count = 0;
    let mut peak_bytes = 0;
    for patch in 0..80 {
        let payload = if patch < 64 {
            "small 界".into()
        } else {
            "界".repeat(340)
        };
        let lines = if patch < 64 { 1 } else { 200 };
        let mut data = format!(
            "--- a/soak-{iteration}-{patch}\n+++ b/soak-{iteration}-{patch}\n@@ -0,0 +1,{lines} @@\n"
        );
        for _ in 0..lines {
            data.push('+');
            data.push_str(&payload);
            data.push('\n');
        }
        data.push_str("shell prompt\n");
        // Deliberately split UTF-8 scalars and headers across reads.
        for chunk in data.as_bytes().chunks(257) {
            detector.feed_bytes(chunk, |diff| {
                completed += 1;
                assert_eq!(diff.filename, format!("soak-{iteration}-{patch}"));
                assert_eq!(diff.id, (iteration * 80 + completed) as u64);
            });
        }
        let (count, bytes) = detector.history_diagnostics();
        peak_count = peak_count.max(count);
        peak_bytes = peak_bytes.max(bytes);
        assert!(count <= 64 && bytes <= 4 * 1024 * 1024);
    }
    let retained = detector.history_diagnostics().0;
    let evicted = before + completed - retained;
    assert_eq!(completed, 80);
    assert_eq!(peak_count, 64);
    assert!(peak_bytes >= 3 * 1024 * 1024);
    assert!(evicted > 0);
    assert!(
        detector
            .detected_by_id((iteration * 80 + 1) as u64)
            .is_none()
    );
    assert!(
        detector
            .take_notices()
            .all(|notice| notice.contains("evicted"))
    );
    (completed, evicted, retained, peak_count, peak_bytes)
}

#[cfg(target_os = "linux")]
#[test]
fn persistent_diff_history_soak_exercises_count_and_byte_eviction() {
    let mut detector = crate::terminal::diff_detector::DiffDetector::new();
    for iteration in 0..3 {
        soak_diff_history(&mut detector, iteration);
    }
}

#[test]
#[ignore = "run with scripts/run_benchmarks.py; requires a release build"]
fn release_benchmark_suite() {
    use crate::app::App;
    use crate::config::Config;
    use crate::panes::{PaneId, PaneTree, SplitDirection};
    use crate::renderer::benchmark_headless_renderer;
    use crate::session::transcript::benchmark_slow_transcript_enqueue;
    use crate::terminal::search::{
        MAX_RETAINED_SEARCH_MATCHES, MAX_SEARCH_RESULT_INDEX_BYTES, Search,
    };
    use crate::ui::unicode_picker::benchmark_unicode_picker;
    use std::fmt::Write as _;

    require_release_profile();

    let mut startup_samples = Vec::with_capacity(5);
    for _ in 0..5 {
        let started = Instant::now();
        let app = App::new_headless(Config::default()).unwrap();
        startup_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        drop(app);
    }
    emit_benchmark(
        "latency.headless_controller_startup",
        "ms",
        startup_samples,
        "construct application controller without window, PTY, or clipboard",
        None,
    );

    let diff = crate::terminal::diff_detector::performance::benchmark_streaming_diff();
    for (name, unit, samples, workload) in [
        (
            "latency.diff_observer.plain_4mib",
            "ms",
            diff.plain_ms,
            "warm observer-only scan of 4 MiB plain 128-byte LF lines in 4096-byte reads; no captures",
        ),
        (
            "latency.diff_observer.patches_bulk",
            "ms",
            diff.bulk_ms,
            "warm observer-only capture of 64 small and 16 200-line Unicode patches with SGR/OSC/CRLF in 4096-byte reads; includes completion and history eviction",
        ),
        (
            "latency.diff_observer.patches_fragmented",
            "ms",
            diff.fragmented_ms,
            "warm observer-only capture of the identical 80-patch SGR/OSC/CRLF stream in 257-byte reads; includes completion and history eviction",
        ),
        (
            "memory.diff_observer.retained_history",
            "bytes",
            diff.retained_bytes,
            "independently recounted final history storage after the 80-patch stream; capacities and conservative inline charges, not allocator traffic or RSS",
        ),
    ] {
        emit_benchmark(name, unit, samples, workload, None);
    }

    let unicode_picker = benchmark_unicode_picker();
    emit_benchmark(
        "latency.unicode_picker_exhaustive_batch",
        "ms",
        unicode_picker.exhaustive_batch_ms,
        "v1; one warm Unicode 17 / CLDR 48 batch: letter, black, arabic letter, mathematical symbol, cjk unified; bounded results and owned-byte checks",
        None,
    );
    emit_benchmark(
        "latency.unicode_annotation_index_validation",
        "ms",
        unicode_picker.annotation_validation_ms,
        "zero-copy structural and Unicode 17 grapheme validation of the embedded CLDR 48 English annotation index",
        None,
    );
    emit_benchmark(
        "latency.unicode_picker_keyword_query",
        "ms",
        unicode_picker.keyword_query_ms,
        "warm end-to-end Unicode canonical-name plus CLDR keyword queries with bounded ranking and deduplication",
        None,
    );
    emit_benchmark(
        "memory.unicode_annotation_static_bytes",
        "bytes",
        unicode_picker.annotation_static_bytes,
        "complete zero-copy embedded CLDR annotation binary including header, offsets, and UTF-8 pool",
        None,
    );
    emit_benchmark(
        "memory.unicode_picker_keyword_results",
        "bytes",
        unicode_picker.result_owned_bytes,
        "owned candidate/action/command bytes retained for each bounded CLDR keyword result set",
        None,
    );

    fn search_samples(lines: usize) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
        let mut terminal = Terminal::new(100, 40);
        terminal.grid_mut().set_scrollback_limit(lines);
        let mut text = String::with_capacity(lines * 36);
        for line in 0..lines {
            let marker = if line % 100 == 0 {
                "needle"
            } else {
                "haystack"
            };
            writeln!(text, "{line:06} {marker} searchable content\r").unwrap();
        }
        terminal.process(text.as_bytes());

        let mut total_samples = Vec::with_capacity(5);
        let mut dispatch_samples = Vec::with_capacity(5);
        let mut extension_dispatch_samples = Vec::with_capacity(5);
        for _ in 0..5 {
            let mut search = Search::new();
            search.start();
            let started = Instant::now();
            for character in "needle".chars() {
                search.push_char(character, terminal.grid());
            }
            dispatch_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
            search.finish(terminal.grid());
            total_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
            assert_eq!(search.match_count(), lines / 100);

            // Model a paused typist: the broad first character has completed
            // before the next key arrives. This catches candidate-row copies
            // or result cleanup that scale badly on the keyboard path.
            let mut broad_search = Search::new();
            broad_search.start();
            broad_search.push_char('n', terminal.grid());
            broad_search.finish(terminal.grid());
            assert_eq!(broad_search.matching_line_count(), lines);
            let extension_started = Instant::now();
            broad_search.push_char('e', terminal.grid());
            extension_dispatch_samples.push(extension_started.elapsed().as_secs_f64() * 1_000.0);
            broad_search.finish(terminal.grid());
            assert_eq!(broad_search.match_count(), lines / 100);
        }
        (total_samples, dispatch_samples, extension_dispatch_samples)
    }

    let (search_10k, search_dispatch_10k, search_extension_dispatch_10k) = search_samples(10_000);
    let (search_100k, search_dispatch_100k, search_extension_dispatch_100k) =
        search_samples(100_000);

    emit_benchmark(
        "interaction.search.10k",
        "ms",
        search_10k,
        "dispatch and complete a cooperative six-character literal query over 10,000 lines",
        None,
    );
    emit_benchmark(
        "interaction.search.100k",
        "ms",
        search_100k,
        "dispatch and complete a cooperative six-character literal query over 100,000 lines",
        None,
    );
    emit_benchmark(
        "interaction.search_dispatch.10k",
        "ms",
        search_dispatch_10k,
        "keyboard-path dispatch for a six-character literal query with 10,000 lines",
        None,
    );
    emit_benchmark(
        "interaction.search_dispatch.100k",
        "ms",
        search_dispatch_100k,
        "keyboard-path dispatch for a six-character literal query with 100,000 lines",
        None,
    );
    emit_benchmark(
        "interaction.search_extension_dispatch.10k",
        "ms",
        search_extension_dispatch_10k,
        "keyboard-path literal extension after a completed broad 10,000-line result",
        None,
    );
    emit_benchmark(
        "interaction.search_extension_dispatch.100k",
        "ms",
        search_extension_dispatch_100k,
        "keyboard-path literal extension after a completed broad 100,000-line result",
        None,
    );

    fn dense_search_samples() -> (Vec<f64>, Vec<f64>) {
        let lines = 10_000;
        let mut terminal = Terminal::new(100, 40);
        terminal.grid_mut().set_scrollback_limit(lines);
        let dense_line = "a".repeat(80);
        let mut text = String::with_capacity(lines * 82);
        for _ in 0..lines {
            writeln!(text, "{dense_line}\r").unwrap();
        }
        terminal.process(text.as_bytes());

        let mut time_samples = Vec::with_capacity(5);
        let mut memory_samples = Vec::with_capacity(5);
        for _ in 0..5 {
            let mut search = Search::new();
            search.start();
            let started = Instant::now();
            search.push_char('a', terminal.grid());
            search.finish(terminal.grid());
            time_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
            memory_samples.push(search.result_index_memory_bytes() as f64);
            assert!(search.results_truncated());
            assert_eq!(search.retained_match_count(), MAX_RETAINED_SEARCH_MATCHES);
            assert!(search.match_count() >= lines * 80);
            assert_eq!(search.matching_line_count(), lines);
            assert!(search.result_index_memory_bytes() <= MAX_SEARCH_RESULT_INDEX_BYTES);
        }
        (time_samples, memory_samples)
    }

    let (dense_search_time, dense_search_memory) = dense_search_samples();
    emit_benchmark(
        "interaction.search.10k_dense",
        "ms",
        dense_search_time,
        "complete a one-character query with 800,000 occurrences across 10,000 lines",
        None,
    );
    emit_benchmark(
        "memory.search_results.10k_dense_estimated_bytes",
        "bytes",
        dense_search_memory,
        "retained occurrence and complete matching-row index capacity for 800,000 occurrences",
        None,
    );

    fn pane_layout_samples(panes: u32) -> Vec<f64> {
        let mut tree = PaneTree::new(PaneId(0));
        for pane in 1..panes {
            assert!(tree.split(
                PaneId(pane - 1),
                PaneId(pane),
                if pane % 2 == 0 {
                    SplitDirection::Horizontal
                } else {
                    SplitDirection::Vertical
                },
            ));
        }
        let bounds = Bounds::new(0.0, 0.0, 1_920.0, 1_080.0);
        // Prime layout allocation/code paths before measuring repeatability;
        // the variation budget compares steady samples, not one cold call
        // against four warm calls.
        for _ in 0..1_000 {
            assert_eq!(tree.calculate_layout(bounds).len(), panes as usize);
            assert_eq!(
                tree.get_dividers(bounds).len(),
                panes.saturating_sub(1) as usize
            );
        }
        let mut samples = Vec::with_capacity(5);
        for _ in 0..5 {
            let started = Instant::now();
            for _ in 0..1_000 {
                assert_eq!(tree.calculate_layout(bounds).len(), panes as usize);
                assert_eq!(
                    tree.get_dividers(bounds).len(),
                    panes.saturating_sub(1) as usize
                );
            }
            samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        }
        samples
    }

    emit_benchmark(
        "interaction.pane_layout.8",
        "ms",
        pane_layout_samples(8),
        "1,000 layout and divider calculations for 8 panes",
        None,
    );
    emit_benchmark(
        "interaction.pane_layout.64",
        "ms",
        pane_layout_samples(64),
        "1,000 layout and divider calculations for 64 panes",
        None,
    );

    let transcript = benchmark_slow_transcript_enqueue().unwrap();
    emit_benchmark(
        "latency.transcript_enqueue_under_slow_disk",
        "ms",
        transcript.samples_ms,
        "enqueue 1,000 interleaved input/output transcript entries with an injected 2 ms writer",
        None,
    );
    emit_benchmark(
        "transcript.slow_disk_dropped_entries",
        "entries",
        transcript.dropped_entries,
        "entries explicitly shed by the bounded queue under the slow-writer workload",
        None,
    );

    if std::path::Path::new("/bin/true").exists() {
        let mut samples = Vec::with_capacity(5);
        for _ in 0..5 {
            let mut tabs = TabManager::new(
                100,
                40,
                ShellConfig::default(),
                10_000,
                termeric_core::DEFAULT_SCROLLBACK_MEMORY_BYTES,
            );
            let started = Instant::now();
            for index in 0..8 {
                tabs.new_tab_with_command(
                    "/bin/true",
                    &[],
                    None,
                    Some(&format!("benchmark-{index}")),
                    None,
                    HashMap::new(),
                )
                .unwrap();
            }
            samples.push(started.elapsed().as_secs_f64() * 1_000.0);
            assert_eq!(tabs.count(), 8);
        }
        emit_benchmark(
            "latency.tabs.create_8",
            "ms",
            samples,
            "create eight real PTY-backed tabs running /bin/true",
            None,
        );
    }

    match benchmark_headless_renderer().unwrap() {
        Some(result) => {
            emit_benchmark(
                "renderer.frame_build_upload_submit",
                "ms",
                result.frame_ms,
                "120x40 text grid plus styled rectangles rendered offscreen",
                Some(&result.adapter),
            );
            emit_benchmark(
                "renderer.glyph_cache_warm",
                "ms",
                result.glyph_warm_ms,
                "clear and rasterize 80 representative glyphs",
                Some(&result.adapter),
            );
            emit_benchmark(
                "renderer.glyph_cache_hit_rate",
                "percent",
                result.glyph_cache_hit_percent,
                "atlas hit rate for 100 repeats of the representative warmed glyph set",
                Some(&result.adapter),
            );
            emit_benchmark(
                "latency.synthetic_key_to_gpu_submit",
                "ms",
                result.key_to_submit_ms,
                "terminal input mutation through an offscreen rendered frame",
                Some(&result.adapter),
            );
            emit_benchmark(
                "renderer.steady_frame_buffer_allocations",
                "allocations",
                vec![result.steady_frame_buffer_allocations as f64; 5],
                "new text/rectangle GPU buffers after equal-size warmup",
                Some(&result.adapter),
            );
            for (name, samples, workload) in [
                (
                    "renderer.row_cache.static",
                    result.row_cache_static_ms,
                    "40 unchanged cached terminal rows",
                ),
                (
                    "renderer.row_cache.one_row_update",
                    result.row_cache_one_row_ms,
                    "one changed row among 40 cached terminal rows",
                ),
                (
                    "renderer.row_cache.scroll",
                    result.row_cache_scroll_ms,
                    "40 remapped rows during a scroll",
                ),
                (
                    "renderer.row_cache.search_heavy",
                    result.row_cache_search_heavy_ms,
                    "search-highlight changes on half of 40 rows",
                ),
                (
                    "renderer.row_cache.unicode",
                    result.row_cache_unicode_ms,
                    "40 cached rows containing wide and non-ASCII glyphs",
                ),
                (
                    "renderer.row_cache.multi_pane",
                    result.row_cache_multi_pane_ms,
                    "unchanged lookup across eight 40-row panes",
                ),
            ] {
                emit_benchmark(name, "ms", samples, workload, Some(&result.adapter));
            }
            emit_benchmark(
                "renderer.row_cache.one_row_rebuilds",
                "rows",
                result.row_cache_one_row_rebuilds,
                "rows rebuilt after a one-row signature change",
                Some(&result.adapter),
            );
            emit_benchmark(
                "renderer.row_cache.steady_allocations",
                "allocations",
                result.row_cache_steady_allocations,
                "new cached-row GPU buffers on unchanged frames",
                Some(&result.adapter),
            );
            emit_benchmark(
                "renderer.ligature.code_run_build",
                "ms",
                result.ligature_code_ms,
                "shape and build 1,000 code-heavy row runs after cache warmup",
                Some(&result.adapter),
            );
            emit_benchmark(
                "renderer.ligature.shaping_cache_hit_rate",
                "percent",
                result.ligature_cache_hit_percent,
                "shaping-cache hit rate for unchanged code-heavy row runs",
                Some(&result.adapter),
            );
            let pressure_context = format!(
                "{}; fixture=shaping-pressure-v1; font_sha256={}",
                result.adapter, result.shaping_pressure.font_sha256,
            );
            for (name, unit, samples, workload) in [
                (
                    "renderer.shaping_pressure.rebuild",
                    "ms",
                    result.shaping_pressure.pressure_ms,
                    "128 unique 2048-glyph Fira Mono runs; warmed atlas; production 8 MiB cache churn",
                ),
                (
                    "renderer.shaping_pressure.retained_reference",
                    "ms",
                    result.shaping_pressure.reference_ms,
                    "identical 128 unique 2048-glyph Fira Mono runs; warmed atlas; test-only 64 MiB retained cache",
                ),
                (
                    "memory.shaping_pressure.retained_bytes",
                    "bytes",
                    result.shaping_pressure.pressure_bytes,
                    "owned run-cache bytes after the production 8 MiB pressure pass; not peak or RSS",
                ),
                (
                    "memory.shaping_pressure.reference_bytes",
                    "bytes",
                    result.shaping_pressure.reference_bytes,
                    "owned run-cache bytes after the test-only 64 MiB retained-reference pass; not peak or RSS",
                ),
                (
                    "renderer.shaping_pressure.evictions",
                    "entries",
                    result.shaping_pressure.pressure_evictions,
                    "capacity-evicted run entries during the production 8 MiB pressure pass",
                ),
                (
                    "renderer.shaping_pressure.rasterizations",
                    "glyphs",
                    result.shaping_pressure.pressure_rasterizations,
                    "physical glyph rasterizations during the production pressure pass with warmed atlas",
                ),
            ] {
                emit_benchmark(name, unit, samples, workload, Some(&pressure_context));
            }
        }
        None => {
            for (name, unit) in [
                ("renderer.frame_build_upload_submit", "ms"),
                ("renderer.glyph_cache_warm", "ms"),
                ("renderer.glyph_cache_hit_rate", "percent"),
                ("latency.synthetic_key_to_gpu_submit", "ms"),
                ("renderer.steady_frame_buffer_allocations", "allocations"),
                ("renderer.row_cache.static", "ms"),
                ("renderer.row_cache.one_row_update", "ms"),
                ("renderer.row_cache.scroll", "ms"),
                ("renderer.row_cache.search_heavy", "ms"),
                ("renderer.row_cache.unicode", "ms"),
                ("renderer.row_cache.multi_pane", "ms"),
                ("renderer.row_cache.one_row_rebuilds", "rows"),
                ("renderer.row_cache.steady_allocations", "allocations"),
                ("renderer.ligature.code_run_build", "ms"),
                ("renderer.ligature.shaping_cache_hit_rate", "percent"),
                ("renderer.shaping_pressure.rebuild", "ms"),
                ("renderer.shaping_pressure.retained_reference", "ms"),
                ("memory.shaping_pressure.retained_bytes", "bytes"),
                ("memory.shaping_pressure.reference_bytes", "bytes"),
                ("renderer.shaping_pressure.evictions", "entries"),
                ("renderer.shaping_pressure.rasterizations", "glyphs"),
            ] {
                emit_benchmark(
                    name,
                    unit,
                    Vec::new(),
                    "skipped: no headless WGPU adapter",
                    None,
                );
            }
        }
    }
}
