//! Cross-module runtime fixtures. These deliberately exercise real public
//! boundaries instead of replacing terminal, tab, or plugin behavior with mocks.

use crate::config::ShellConfig;
use crate::harvest::{HarvestMode, HarvestSource, HarvestTerminalState};
use crate::tabs::{TabManager, TabType};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use termeric_core::{CellFlags, Terminal};

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
            pane_id: None,
        },
        &regions[0],
    );
    assert_eq!(harvest.cellar.len(), 1);
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

    let mut tabs = TabManager::new(80, 24, ShellConfig::default(), 100);
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
        tabs.process_all_events();
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
fn linux_headless_gpu_render_smoke_when_requested() {
    if std::env::var("TERMERIC_GPU_SMOKE").as_deref() != Ok("1") {
        eprintln!("skipping GPU smoke: set TERMERIC_GPU_SMOKE=1 (CI uses Vulkan + llvmpipe)");
        return;
    }

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .or_else(|| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
    })
    .expect("TERMERIC_GPU_SMOKE=1 but no headless GPU adapter was found");
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
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
