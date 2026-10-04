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

mod benchmarks;
mod soak;

/// `sh -c` arguments for a child that stays alive through a test without
/// printing anything: a real PTY whose process is still running when the
/// test looks.
pub(crate) fn sleeping_args() -> Vec<String> {
    vec!["-c".into(), "sleep 2".into()]
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
