//! The headless renderer benchmark behind the release benchmark suite.

use super::headless_tests::queue_test_run;
use super::*;

pub(crate) struct HeadlessRendererBenchmark {
    pub adapter: String,
    pub frame_ms: Vec<f64>,
    pub glyph_warm_ms: Vec<f64>,
    pub glyph_cache_hit_percent: Vec<f64>,
    pub steady_frame_buffer_allocations: u64,
    pub ligature_code_ms: Vec<f64>,
    pub ligature_cache_hit_percent: Vec<f64>,
    pub shaping_pressure: glyph_cache::pressure_benchmark::PressureBenchmark,
}

/// Exercise Termeric's real glyph, text, rectangle, upload, and draw pipelines
/// against an offscreen texture. Presentation and window-system latency remain
/// part of the optional process-level benchmark lane.
pub(crate) fn benchmark_headless_renderer() -> Result<Option<HeadlessRendererBenchmark>> {
    use std::time::Instant;

    const WIDTH: u32 = 1_280;
    const HEIGHT: u32 = 720;
    const COLS: usize = 120;
    const ROWS: usize = 40;
    const SAMPLES: usize = 5;
    const FRAMES_PER_SAMPLE: usize = 10;
    const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

    let instance_descriptor = InstanceDescriptor::from_env_or_default();
    let enabled_backends = instance_descriptor.backends;
    let instance = Instance::new(&instance_descriptor);
    let Some(adapter) = discover_adapter(
        &instance,
        enabled_backends,
        None,
        PowerPreference::LowPower,
        true,
    )
    .or_else(|| {
        discover_adapter(
            &instance,
            enabled_backends,
            None,
            PowerPreference::LowPower,
            false,
        )
    }) else {
        return Ok(None);
    };
    let adapter_info = adapter.get_info();
    let adapter_name = format!(
        "name={:?}; backend={:?}; device_type={:?}; driver={:?}; driver_info={:?}; vendor={:#06x}; device={:#06x}",
        adapter_info.name,
        adapter_info.backend,
        adapter_info.device_type,
        adapter_info.driver,
        adapter_info.driver_info,
        adapter_info.vendor,
        adapter_info.device,
    );
    let (device, queue) = pollster::block_on(adapter.request_device(
        &DeviceDescriptor {
            required_features: Features::empty(),
            required_limits: Limits::downlevel_defaults(),
            memory_hints: MemoryHints::default(),
            label: Some("Termeric benchmark device"),
        },
        None,
    ))?;

    let texture = device.create_texture(&TextureDescriptor {
        label: Some("Termeric benchmark target"),
        size: Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: FORMAT,
        usage: TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&TextureViewDescriptor::default());
    let mut glyph_cache = GlyphCache::for_test(&device, &queue, 14.0, false)?;
    let mut text = TextPipeline::new(&device, FORMAT, &glyph_cache)?;
    let mut rect = RectPipeline::new(&device, FORMAT)?;
    text.resize(&queue, WIDTH, HEIGHT);
    rect.resize(&queue, WIDTH, HEIGHT);
    let (cell_width, cell_height) = glyph_cache.cell_size();
    let glyphs: Vec<char> =
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789[]{}()<>+-=*/_界αβ"
            .chars()
            .collect();

    let mut glyph_warm_ms = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        glyph_cache.clear(&queue);
        let started = Instant::now();
        for &character in &glyphs {
            let _ = glyph_cache.get_or_insert(character, &queue);
        }
        glyph_warm_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    let before_common_hits = glyph_cache.atlas_metrics();
    for _ in 0..100 {
        for &character in &glyphs {
            let _ = glyph_cache.get_or_insert(character, &queue);
        }
    }
    let after_common_hits = glyph_cache.atlas_metrics();
    let common_hits = after_common_hits.hits - before_common_hits.hits;
    let common_misses = after_common_hits.misses - before_common_hits.misses;
    let common_hit_percent = common_hits as f64 / (common_hits + common_misses) as f64 * 100.0;
    assert!(common_hit_percent >= 99.0);
    let glyph_cache_hit_percent = vec![common_hit_percent; SAMPLES];

    let submit_frame =
        |text: &mut TextPipeline, rect: &mut RectPipeline, glyph_cache: &mut GlyphCache| -> f64 {
            let started = Instant::now();
            text.clear();
            rect.clear();
            for row in 0..ROWS {
                for col in 0..COLS {
                    let character = glyphs[(row * COLS + col) % glyphs.len()];
                    text.push_char(
                        character,
                        col as f32 * cell_width,
                        row as f32 * cell_height,
                        [0.92, 0.88, 0.78, 1.0],
                    );
                    if (row + col) % 23 == 0 {
                        rect.push_rect(
                            col as f32 * cell_width,
                            row as f32 * cell_height,
                            cell_width,
                            cell_height,
                            [0.18, 0.12, 0.08, 1.0],
                        );
                    }
                }
            }
            text.upload(&device, &queue, glyph_cache);
            rect.upload(&device, &queue);
            let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
                label: Some("Termeric benchmark encoder"),
            });
            {
                let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                    label: Some("Termeric benchmark pass"),
                    color_attachments: &[Some(RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: Operations {
                            load: LoadOp::Clear(Color::BLACK),
                            store: StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                rect.draw_layer(&mut pass, 0);
                text.draw_layer(&mut pass, 0);
            }
            let submission = queue.submit([encoder.finish()]);
            let _ = device.poll(Maintain::wait_for(submission));
            started.elapsed().as_secs_f64() * 1_000.0
        };

    let _ = submit_frame(&mut text, &mut rect, &mut glyph_cache);
    let allocations_after_warmup = text.buffer_allocations() + rect.buffer_allocations();
    let mut frame_ms = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        for _ in 0..FRAMES_PER_SAMPLE {
            let _ = submit_frame(&mut text, &mut rect, &mut glyph_cache);
        }
        frame_ms.push(started.elapsed().as_secs_f64() * 1_000.0 / FRAMES_PER_SAMPLE as f64);
    }
    let allocations_after_steady_frames = text.buffer_allocations() + rect.buffer_allocations();
    let steady_frame_buffer_allocations =
        allocations_after_steady_frames - allocations_after_warmup;
    assert_eq!(steady_frame_buffer_allocations, 0);

    // Measure the row-run path on code-heavy tokens after warming every
    // distinct content key. This isolates shaping/cache cost from GPU submit.
    glyph_cache.set_ligatures(true);
    let code_tokens = [
        "let", "result", "left", "!=", "right", "=>", "compute", "value", "===", "expected",
    ];
    let code_clip = TextClip {
        left: 0.0,
        top: 0.0,
        right: WIDTH as f32,
        bottom: HEIGHT as f32,
    };
    let build_code_tokens =
        |text: &mut TextPipeline, glyph_cache: &mut GlyphCache, repetitions: usize| {
            for _ in 0..repetitions {
                for token in code_tokens {
                    queue_test_run(text, token, cell_width, code_clip);
                }
            }
            let queued = text.take_queued_since(0);
            TextPipeline::build_batch(&queued, &queue, glyph_cache)
        };
    let _ = build_code_tokens(&mut text, &mut glyph_cache, 1);
    let before = glyph_cache.shaping_cache_metrics();
    let mut ligature_code_ms = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let batch = build_code_tokens(&mut text, &mut glyph_cache, 100);
        assert!(!batch.vertices.is_empty());
        ligature_code_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    let after = glyph_cache.shaping_cache_metrics();
    let hits = after.hits - before.hits;
    let misses = after.misses - before.misses;
    let hit_percent = hits as f64 / (hits + misses) as f64 * 100.0;
    assert!(hit_percent >= 99.0);
    let ligature_cache_hit_percent = vec![hit_percent; SAMPLES];
    let shaping_pressure = glyph_cache::pressure_benchmark::measure(&device, &queue)?;

    Ok(Some(HeadlessRendererBenchmark {
        adapter: adapter_name,
        frame_ms,
        glyph_warm_ms,
        glyph_cache_hit_percent,
        steady_frame_buffer_allocations,
        ligature_code_ms,
        ligature_cache_hit_percent,
        shaping_pressure,
    }))
}
