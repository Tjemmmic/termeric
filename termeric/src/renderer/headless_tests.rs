//! Headless GPU smokes of the real atlas, text, rectangle, and image
//! pipelines, run by `test_support::gpu_render_smoke`.

use super::*;

/// Run one render pass into a fresh `width`×`height` target cleared to
/// `clear`, then read the target back as tightly packed rows.
#[cfg(test)]
pub(super) fn render_offscreen(
    device: &Device,
    queue: &Queue,
    format: TextureFormat,
    (width, height): (u32, u32),
    clear: wgpu::Color,
    draw: impl FnOnce(&mut RenderPass<'_>),
) -> Result<Vec<u8>> {
    anyhow::ensure!(
        (width * 4).is_multiple_of(COPY_BYTES_PER_ROW_ALIGNMENT),
        "readback rows of {width} pixels are not {COPY_BYTES_PER_ROW_ALIGNMENT}-byte aligned"
    );
    let extent = Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let target = device.create_texture(&TextureDescriptor {
        label: Some("Headless test target"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("Headless test readback"),
        size: u64::from(width * height * 4),
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let view = target.create_view(&TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("Headless test encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("Headless test pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(clear),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        draw(&mut pass);
    }
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
        },
        extent,
    );
    let submission = queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback.slice(..).map_async(MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    let _ = device.poll(Maintain::wait_for(submission));
    receiver.recv()?.map_err(|error| anyhow::anyhow!(error))?;
    let pixels = readback.slice(..).get_mapped_range().to_vec();
    readback.unmap();
    Ok(pixels)
}

#[cfg(test)]
pub(super) const HEADLESS_IMAGE_WIDTH: u32 = 64;
#[cfg(test)]
const HEADLESS_IMAGE_CELL_HEIGHT: u32 = 4;
#[cfg(test)]
const HEADLESS_IMAGE_ROWS: u32 = 52;
#[cfg(test)]
pub(super) const HEADLESS_IMAGE_HEIGHT: u32 = HEADLESS_IMAGE_CELL_HEIGHT * HEADLESS_IMAGE_ROWS;

#[cfg(test)]
fn headless_image_row_color(row: u32) -> [u8; 4] {
    [
        32 + (row * 3 % 200) as u8,
        48 + (row * 5 % 180) as u8,
        64 + (row * 7 % 160) as u8,
        255,
    ]
}

#[cfg(test)]
fn headless_image_fixture() -> termeric_core::StoredImage {
    let source_width = HEADLESS_IMAGE_WIDTH / 2;
    let mut data = Vec::with_capacity((source_width * HEADLESS_IMAGE_HEIGHT * 4) as usize);
    for pixel_row in 0..HEADLESS_IMAGE_HEIGHT {
        let color = headless_image_row_color(pixel_row / HEADLESS_IMAGE_CELL_HEIGHT);
        for _ in 0..source_width {
            data.extend_from_slice(&color);
        }
    }
    termeric_core::StoredImage {
        id: termeric_core::ImageId(7),
        width: source_width,
        height: HEADLESS_IMAGE_HEIGHT,
        data: data.into(),
        revision: 1,
    }
}

#[cfg(test)]
fn headless_image_presentations(
    image: &termeric_core::StoredImage,
    row_origin: i64,
) -> Vec<termeric_core::PresentationImage> {
    let mut presentations = Vec::with_capacity(HEADLESS_IMAGE_ROWS as usize + 1);
    presentations.push(termeric_core::PresentationImage {
        placement: termeric_core::ImagePlacement {
            image_id: image.id,
            placement_id: Some(1),
            col: 0,
            row: row_origin,
            alternate_screen: false,
            source_x: 0,
            source_y: 0,
            source_width: Some(image.width),
            source_height: Some(image.height),
            x_offset: 0,
            y_offset: 0,
            width: Some(4),
            height: Some(HEADLESS_IMAGE_ROWS),
            pixel_width: Some(image.width),
            pixel_height: Some(image.height),
            z_index: -1,
        },
        image: image.clone(),
        pixel_rect: None,
        source_uv: None,
    });
    for row in 0..HEADLESS_IMAGE_ROWS {
        presentations.push(termeric_core::PresentationImage {
            placement: termeric_core::ImagePlacement {
                image_id: image.id,
                placement_id: Some(2),
                col: 4,
                row: row_origin + i64::from(row),
                alternate_screen: false,
                source_x: 0,
                source_y: row * HEADLESS_IMAGE_CELL_HEIGHT,
                source_width: Some(image.width),
                source_height: Some(HEADLESS_IMAGE_CELL_HEIGHT),
                x_offset: 0,
                y_offset: 0,
                width: Some(4),
                height: Some(1),
                pixel_width: Some(image.width),
                pixel_height: Some(HEADLESS_IMAGE_CELL_HEIGHT),
                // Kitty Unicode placeholders are specified as text-backed
                // virtual placements and render below the placeholder text.
                z_index: -1,
            },
            image: image.clone(),
            pixel_rect: Some([
                0.0,
                0.0,
                image.width as f32,
                HEADLESS_IMAGE_CELL_HEIGHT as f32,
            ]),
            source_uv: Some([
                0.0,
                row as f32 / HEADLESS_IMAGE_ROWS as f32,
                1.0,
                (row + 1) as f32 / HEADLESS_IMAGE_ROWS as f32,
            ]),
        });
    }
    presentations
}

#[cfg(test)]
pub(super) fn render_headless_image_state(
    device: &Device,
    queue: &Queue,
    pipeline: &mut ImagePipeline,
    key: (usize, termeric_core::ImageId),
    presentations: &[termeric_core::PresentationImage],
    scroll_offset: usize,
) -> Result<Vec<u8>> {
    pipeline.clear();
    for presentation in presentations {
        if let Some(geometry) = presentation_geometry(
            presentation,
            8.0,
            HEADLESS_IMAGE_CELL_HEIGHT as f32,
            scroll_offset,
            Some((HEADLESS_IMAGE_WIDTH as f32, HEADLESS_IMAGE_HEIGHT as f32)),
        ) {
            pipeline.push_image(
                key,
                geometry.x,
                geometry.y,
                geometry.width,
                geometry.height,
                geometry.u_min,
                geometry.v_min,
                geometry.u_max,
                geometry.v_max,
                1.0,
                presentation.placement.z_index,
            );
        }
    }
    pipeline.upload(device, queue);

    render_offscreen(
        device,
        queue,
        TextureFormat::Rgba8Unorm,
        (HEADLESS_IMAGE_WIDTH, HEADLESS_IMAGE_HEIGHT),
        wgpu::Color::BLACK,
        |pass| {
            pipeline.draw_below_text(pass);
            pipeline.draw_above_text(pass);
        },
    )
}

#[cfg(test)]
fn verify_headless_image_state(
    pixels: &[u8],
    source_row_offset: u32,
    visible_rows: u32,
    label: &str,
) -> Result<()> {
    anyhow::ensure!(
        pixels.len() == (HEADLESS_IMAGE_WIDTH * HEADLESS_IMAGE_HEIGHT * 4) as usize,
        "{label} graphics readback has the wrong size"
    );
    let half_width = HEADLESS_IMAGE_WIDTH / 2;
    let visible_height = visible_rows * HEADLESS_IMAGE_CELL_HEIGHT;
    for y in 0..HEADLESS_IMAGE_HEIGHT {
        for x in 0..half_width {
            let left_offset = ((y * HEADLESS_IMAGE_WIDTH + x) * 4) as usize;
            let right_offset = ((y * HEADLESS_IMAGE_WIDTH + x + half_width) * 4) as usize;
            let left = &pixels[left_offset..left_offset + 4];
            let right = &pixels[right_offset..right_offset + 4];
            if y < visible_height {
                let expected =
                    headless_image_row_color(source_row_offset + y / HEADLESS_IMAGE_CELL_HEIGHT);
                for channel in 0..4 {
                    anyhow::ensure!(
                        left[channel].abs_diff(expected[channel]) <= 3,
                        "{label} classic image row {} channel {channel} was {} instead of {}",
                        y / HEADLESS_IMAGE_CELL_HEIGHT,
                        left[channel],
                        expected[channel]
                    );
                    anyhow::ensure!(
                        right[channel].abs_diff(expected[channel]) <= 3,
                        "{label} Unicode-placeholder row {} channel {channel} was {} instead of {}",
                        y / HEADLESS_IMAGE_CELL_HEIGHT,
                        right[channel],
                        expected[channel]
                    );
                }
            } else {
                anyhow::ensure!(
                    left[..3].iter().all(|channel| *channel == 0)
                        && right[..3].iter().all(|channel| *channel == 0),
                    "{label} painted outside the expected scrolled image footprint at row {y}"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn expected_headless_image_state(source_row_offset: u32, visible_rows: u32) -> Vec<u8> {
    let mut pixels = vec![0; (HEADLESS_IMAGE_WIDTH * HEADLESS_IMAGE_HEIGHT * 4) as usize];
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[3] = 255;
    }
    let visible_height = visible_rows * HEADLESS_IMAGE_CELL_HEIGHT;
    for y in 0..visible_height {
        let color = headless_image_row_color(source_row_offset + y / HEADLESS_IMAGE_CELL_HEIGHT);
        for x in 0..HEADLESS_IMAGE_WIDTH {
            let offset = ((y * HEADLESS_IMAGE_WIDTH + x) * 4) as usize;
            pixels[offset..offset + 4].copy_from_slice(&color);
        }
    }
    pixels
}

/// Read back complete multi-row classic and Unicode-placeholder images before
/// and after terminal scrolling. Every pixel row must be present in the first
/// frame; scrolling must clip it once and restoring scrollback must reveal the
/// complete image again without incrementally discovering rows.
#[cfg(test)]
pub(crate) fn headless_image_render_smoke(device: &Device, queue: &Queue) -> Result<()> {
    let mut pipeline = ImagePipeline::new(device, TextureFormat::Rgba8Unorm)?;
    pipeline.resize(queue, HEADLESS_IMAGE_WIDTH, HEADLESS_IMAGE_HEIGHT);
    let image = headless_image_fixture();
    let key = (1, image.id);
    pipeline.upload_image(device, queue, key, &image);

    let initial = headless_image_presentations(&image, 0);
    let pixels = render_headless_image_state(device, queue, &mut pipeline, key, &initial, 0)?;
    verify_headless_image_state(&pixels, 0, HEADLESS_IMAGE_ROWS, "initial frame")?;

    let scrolled = headless_image_presentations(&image, -3);
    let pixels = render_headless_image_state(device, queue, &mut pipeline, key, &scrolled, 0)?;
    verify_headless_image_state(&pixels, 3, HEADLESS_IMAGE_ROWS - 3, "output scroll")?;

    let pixels = render_headless_image_state(device, queue, &mut pipeline, key, &scrolled, 3)?;
    verify_headless_image_state(&pixels, 0, HEADLESS_IMAGE_ROWS, "scrollback restore")?;
    image_crop_smoke::run(device, queue)?;
    Ok(())
}

/// Theme hex values and image pixels must reach the frame byte for byte. The
/// target uses the view format selected for an sRGB-only surface, the hardest
/// case: rendering through an sRGB view or sampling an sRGB-decoding texture
/// lightens every mid-tone (Turnip's `#18151A` background presents as
/// `#56515A`), which fails the exact comparisons below.
#[cfg(test)]
pub(crate) fn headless_theme_color_fidelity_smoke(device: &Device, queue: &Queue) -> Result<()> {
    use crate::config::theme::Color as ThemeColor;

    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 4;
    let background = ThemeColor::hex("#18151A");
    let rect_color = ThemeColor::hex("#9B5BA0");
    let image_pixel = [0x7B, 0x9B, 0x5C, 0xFF];
    let format = select_surface_formats(&[TextureFormat::Rgba8UnormSrgb])
        .ok_or_else(|| anyhow::anyhow!("sRGB-only surface produced no render format"))?
        .render;

    let mut rects = RectPipeline::new(device, format)?;
    rects.resize(queue, WIDTH, HEIGHT);
    rects.push_rect(0.0, 0.0, 16.0, HEIGHT as f32, rect_color.to_f32_array());
    rects.upload(device, queue);

    let mut images = ImagePipeline::new(device, format)?;
    images.resize(queue, WIDTH, HEIGHT);
    let image = termeric_core::StoredImage {
        id: termeric_core::ImageId(1),
        width: 1,
        height: 1,
        data: image_pixel.to_vec().into(),
        revision: 1,
    };
    let key = (1, image.id);
    images.upload_image(device, queue, key, &image);
    images.push_image(
        key,
        16.0,
        0.0,
        16.0,
        HEIGHT as f32,
        0.0,
        0.0,
        1.0,
        1.0,
        1.0,
        0,
    );
    images.upload(device, queue);

    let pixels = render_offscreen(
        device,
        queue,
        format,
        (WIDTH, HEIGHT),
        background.to_wgpu(),
        |pass| {
            rects.draw_layer(pass, 0);
            images.draw_above_text(pass);
        },
    )?;

    let rect_rgb = [rect_color.r, rect_color.g, rect_color.b];
    let image_rgb = [image_pixel[0], image_pixel[1], image_pixel[2]];
    let background_rgb = [background.r, background.g, background.b];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (label, expected) = match x {
                0..16 => ("theme rectangle", rect_rgb),
                16..32 => ("image pixel", image_rgb),
                _ => ("theme background", background_rgb),
            };
            let offset = ((y * WIDTH + x) * 4) as usize;
            let actual = &pixels[offset..offset + 3];
            anyhow::ensure!(
                actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| actual.abs_diff(expected) <= 1),
                "{label} at ({x}, {y}) presented {actual:?}, expected {expected:?}"
            );
        }
    }
    Ok(())
}

/// Chrome labels shape in the desktop's UI face through the shared atlas,
/// measure consistently with what is drawn, and land on whole pixels.
#[cfg(test)]
pub(crate) fn headless_ui_text_smoke(device: &Device, queue: &Queue) -> Result<()> {
    let mut glyph_cache = GlyphCache::for_test(device, queue, 16.0, false)?;
    let size = physical_font_size(ui_text::UI_LABEL_POINTS, 1.0);
    let metrics = glyph_cache.ui_font_metrics(size);
    anyhow::ensure!(
        metrics.ascent > metrics.descent && metrics.descent > 0.0,
        "implausible UI font metrics {metrics:?}"
    );
    let width = glyph_cache.ui_text_width("~/dev/termeric", size);
    anyhow::ensure!(width > size * 4.0, "UI label measured only {width}px");
    anyhow::ensure!(
        glyph_cache.ui_text_width("~/dev/termeric", size) == width,
        "UI width memo changed its answer"
    );
    let baseline = metrics.centered_baseline(0.0, 34.0);
    let mut text = TextPipeline::new(device, TextureFormat::Rgba8Unorm, &glyph_cache)?;
    text.push_ui_text("~/dev/termeric", 10.4, baseline, size, [1.0; 4], None);
    let queued = text.take_queued_since(0);
    let batch = TextPipeline::build_batch(&queued, queue, &mut glyph_cache);
    anyhow::ensure!(
        batch.vertices.len() >= 4 * 12,
        "UI label produced {} vertices",
        batch.vertices.len()
    );
    for vertex in &batch.vertices {
        anyhow::ensure!(
            vertex.position[0].fract() == 0.0 && vertex.position[1].fract() == 0.0,
            "UI glyph quad at fractional position {:?}",
            vertex.position
        );
    }
    let right = batch
        .vertices
        .iter()
        .map(|vertex| vertex.position[0])
        .fold(f32::MIN, f32::max);
    anyhow::ensure!(
        (right - (10.0 + width)).abs() <= size,
        "drawn label ends at {right}, measured {width} from 10"
    );
    Ok(())
}

/// Shape and submit representative multi-scalar terminal graphemes through the
/// real text atlas/pipeline. The queued-content assertions act as a small
/// geometry golden: no case may collapse to its first scalar before shaping.
#[cfg(test)]
pub(crate) fn headless_unicode_render_smoke(device: &Device, queue: &Queue) -> Result<()> {
    const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
    const WIDTH: u32 = 512;
    const HEIGHT: u32 = 96;
    let mut glyph_cache = GlyphCache::for_test(device, queue, 14.0, false)?;
    let mut text = TextPipeline::new(device, FORMAT, &glyph_cache)?;
    text.resize(queue, WIDTH, HEIGHT);
    let cases = [
        "e\u{301}",
        "界",
        "☺\u{fe0f}",
        "🇺🇸",
        "👍🏽",
        "👩\u{200d}💻",
        // Complete Unicode output of the VT100 DEC Special Graphics range
        // `_` through `~`; mapping itself is golden-tested in terminal core.
        " ",
        "◆",
        "▒",
        "␉",
        "␌",
        "␍",
        "␊",
        "°",
        "±",
        "␤",
        "␋",
        "┘",
        "┐",
        "┌",
        "└",
        "┼",
        "⎺",
        "⎻",
        "─",
        "⎼",
        "⎽",
        "├",
        "┤",
        "┴",
        "┬",
        "│",
        "≤",
        "≥",
        "π",
        "≠",
        "£",
        "·",
    ];
    for (index, grapheme) in cases.iter().enumerate() {
        text.push_grapheme(
            grapheme,
            8.0 + (index % 16) as f32 * 30.0,
            8.0 + (index / 16) as f32 * 28.0,
            [0.92, 0.88, 0.78, 1.0],
        );
    }
    let queued = text.take_queued_since(0);
    anyhow::ensure!(
        queued.len() == cases.len(),
        "Unicode golden queue lost a cluster"
    );
    for (queued, expected) in queued.iter().zip(cases) {
        match &queued.content {
            text::QueuedContent::Grapheme(actual) => {
                anyhow::ensure!(actual.as_ref() == expected, "Unicode golden was truncated");
            }
            text::QueuedContent::Char(actual) => {
                let mut characters = expected.chars();
                anyhow::ensure!(
                    characters.next() == Some(*actual) && characters.next().is_none(),
                    "multi-scalar golden became one char"
                );
            }
            text::QueuedContent::Run { .. } => {
                anyhow::bail!("standalone Unicode golden unexpectedly became a row run");
            }
            text::QueuedContent::MultiCell { .. } => {
                anyhow::bail!("standalone Unicode golden unexpectedly became a multicell");
            }
            text::QueuedContent::UiText { .. } => {
                anyhow::bail!("terminal Unicode golden unexpectedly became UI text");
            }
        }
    }
    let batch = TextPipeline::build_batch(&queued, queue, &mut glyph_cache);
    anyhow::ensure!(
        !batch.vertices.is_empty(),
        "Unicode golden produced no glyph geometry"
    );

    for (index, grapheme) in cases.iter().enumerate() {
        text.push_grapheme(
            grapheme,
            8.0 + (index % 16) as f32 * 30.0,
            8.0 + (index / 16) as f32 * 28.0,
            [0.92, 0.88, 0.78, 1.0],
        );
    }
    text.upload(device, queue, &mut glyph_cache);
    render_offscreen(
        device,
        queue,
        FORMAT,
        (WIDTH, HEIGHT),
        wgpu::Color::BLACK,
        |pass| text.draw_layer(pass, 0),
    )?;
    Ok(())
}

/// Load a real COLR/CBDT emoji font and carry its RGBA pixels through the
/// dedicated atlas, vertex flag, bind group, and shader compilation path.
#[cfg(test)]
pub(crate) fn headless_color_emoji_render_smoke(device: &Device, queue: &Queue) -> Result<()> {
    let font_path = [
        "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
        "/usr/share/fonts/noto/NotoColorEmoji.ttf",
    ]
    .into_iter()
    .map(std::path::Path::new)
    .find(|path| path.is_file())
    .ok_or_else(|| anyhow::anyhow!("Noto Color Emoji is required for the GPU render smoke"))?;
    let font_data = std::fs::read(font_path)?;
    let mut glyph_cache = GlyphCache::for_test(device, queue, 32.0, false)?;
    glyph_cache.install_test_font(font_data, "Noto Color Emoji", queue);
    glyph_cache.begin_frame();

    let run = glyph_cache
        .shape_grapheme_styled("😀", glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("Noto did not shape the emoji fixture"))?;
    anyhow::ensure!(!run.glyphs.is_empty(), "emoji fixture produced no glyphs");
    anyhow::ensure!(
        run.glyphs.iter().all(|glyph| glyph.entry.color),
        "emoji RGBA data fell back to the monochrome atlas"
    );

    let mut text = TextPipeline::new(device, TextureFormat::Rgba8Unorm, &glyph_cache)?;
    text.resize(queue, 96, 64);
    text.push_grapheme("😀", 8.0, 8.0, [0.1, 0.9, 0.2, 1.0]);
    text.upload(device, queue, &mut glyph_cache);
    let metrics = glyph_cache.atlas_metrics();
    anyhow::ensure!(metrics.color_pages >= 1 && metrics.max_color_pages == 4);
    anyhow::ensure!(metrics.capacity_bytes == 24 * 1024 * 1024);
    Ok(())
}

/// Force the real raster/text path beyond a tiny multi-page atlas. Every
/// scalar must still produce visible geometry, even when all frame-safe pages
/// are pinned and the permanent replacement glyph is required.
#[cfg(test)]
pub(crate) fn headless_glyph_atlas_stress_smoke(device: &Device, queue: &Queue) -> Result<()> {
    const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
    const GLYPHS: usize = 2_048;
    let mut glyph_cache = GlyphCache::for_test(device, queue, 18.0, false)?;
    glyph_cache.use_tiny_test_atlas(64, 3, queue);
    glyph_cache.begin_frame();

    let mut queued = Vec::with_capacity(GLYPHS);
    for index in 0..GLYPHS {
        // Seed enough installed printable glyph/style identities to fill the
        // tiny pages, then broaden the workload with CJK ideographs, which
        // draw through system fallback or the permanent replacement. (A range
        // with invisible scalars such as Hangul fillers would legitimately
        // draw nothing for them on systems with those fonts.)
        let scalar = if index < 94 * 4 {
            0x21 + (index % 94) as u32
        } else {
            0x4E00 + (index % 512) as u32
        };
        let character = char::from_u32(scalar).expect("stress scalar is valid");
        let style_index = (index / 94) % 4;
        queued.push(text::QueuedChar {
            content: text::QueuedContent::Char(character),
            x: (index % 64) as f32 * 8.0,
            y: (index / 64) as f32 * 10.0,
            color: [0.9, 0.8, 0.7, 1.0],
            style: glyph_cache::FontStyle {
                bold: style_index % 2 == 1,
                italic: style_index >= 2,
            },
            clip: None,
        });
    }
    let batch = TextPipeline::build_batch(&queued, queue, &mut glyph_cache);
    anyhow::ensure!(
        batch.vertices.len() == GLYPHS * 4,
        "atlas pressure dropped glyph geometry"
    );
    anyhow::ensure!(
        batch.atlas_pages.len() > 1,
        "stress never crossed an atlas page"
    );
    let metrics = glyph_cache.atlas_metrics();
    anyhow::ensure!(metrics.pages == 3 && metrics.max_pages == 3);
    anyhow::ensure!(
        metrics.allocation_failures > 0,
        "stress never exercised the visible replacement path"
    );

    // A pressure miss is temporary: repeated content in the same frame must
    // reuse the visible replacement without rerasterizing, then retry after
    // frame pins are released. Exercise both the shaped-run and scalar paths.
    let failures_before_probe = metrics.allocation_failures;
    let pressure_run = glyph_cache
        .shape_run_styled("é", glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("pressure-shaped glyph should remain renderable"))?;
    anyhow::ensure!(pressure_run.glyphs.len() == 1);
    let after_shaped_pressure = glyph_cache.atlas_metrics();
    anyhow::ensure!(after_shaped_pressure.allocation_failures > failures_before_probe);
    let shaped_rasterizations = glyph_cache.rasterizations();
    let _ = glyph_cache
        .shape_run_styled("é", glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("deferred shaped glyph should remain renderable"))?;
    anyhow::ensure!(
        glyph_cache.atlas_metrics().allocation_failures
            == after_shaped_pressure.allocation_failures,
        "same-frame shaped pressure rerasterized a known deferred glyph"
    );
    anyhow::ensure!(glyph_cache.rasterizations() == shaped_rasterizations);
    anyhow::ensure!(glyph_cache.shaping_cache_metrics().deferred_entries == 1);

    let _ = glyph_cache
        .get_or_insert_styled('ñ', glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("pressure scalar should remain renderable"))?;
    let after_scalar_pressure = glyph_cache.atlas_metrics();
    let scalar_rasterizations = glyph_cache.rasterizations();
    anyhow::ensure!(
        after_scalar_pressure.allocation_failures > after_shaped_pressure.allocation_failures
    );
    let _ = glyph_cache
        .get_or_insert_styled('ñ', glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("deferred scalar should remain renderable"))?;
    anyhow::ensure!(
        glyph_cache.atlas_metrics().allocation_failures
            == after_scalar_pressure.allocation_failures,
        "same-frame scalar pressure rerasterized a known deferred glyph"
    );
    anyhow::ensure!(glyph_cache.rasterizations() == scalar_rasterizations);

    let evictions_before_retry = after_scalar_pressure.evictions;
    glyph_cache.begin_frame();
    let _ = glyph_cache
        .shape_run_styled("é", glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("next-frame shaped retry should render"))?;
    let _ = glyph_cache
        .get_or_insert_styled('ñ', glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("next-frame scalar retry should render"))?;
    anyhow::ensure!(
        glyph_cache.atlas_metrics().evictions > evictions_before_retry,
        "next-frame pressure retry never reclaimed an unpinned page"
    );

    // Rebuild at a different font size to prove old raster entries are not
    // reused across size/scale identity changes.
    let generation = glyph_cache.atlas_generation();
    glyph_cache.set_font_size(22.0, queue);
    anyhow::ensure!(glyph_cache.atlas_generation() != generation);
    let regular = glyph_cache
        .get_or_insert_styled('W', glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("resized glyph should remain renderable"))?;
    let bold = glyph_cache
        .get_or_insert_styled(
            'W',
            glyph_cache::FontStyle {
                bold: true,
                italic: false,
            },
            queue,
        )
        .ok_or_else(|| anyhow::anyhow!("resized styled glyph should remain renderable"))?;
    anyhow::ensure!(regular.width > 0 && bold.width > 0);
    let before_hits = glyph_cache.rasterizations();
    let again = glyph_cache.get_or_insert_styled('W', glyph_cache::FontStyle::default(), queue);
    anyhow::ensure!(again == Some(regular));
    anyhow::ensure!(glyph_cache.rasterizations() == before_hits);
    let shaped = glyph_cache.shape_run_styled("W", glyph_cache::FontStyle::default(), queue);
    anyhow::ensure!(shaped.is_some());
    let before_shaped_hit = glyph_cache.rasterizations();
    let shaped_again = glyph_cache.shape_run_styled("W", glyph_cache::FontStyle::default(), queue);
    anyhow::ensure!(shaped_again.is_some());
    anyhow::ensure!(glyph_cache.rasterizations() == before_shaped_hit);
    glyph_cache.clear(queue);
    glyph_cache.begin_frame();
    let shared = glyph_cache
        .shape_run_styled("W", glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("cross-path glyph should shape"))?;
    anyhow::ensure!(shared.glyphs.len() == 1);
    let before_cross_path = glyph_cache.rasterizations();
    let scalar = glyph_cache.get_or_insert_styled('W', glyph_cache::FontStyle::default(), queue);
    anyhow::ensure!(scalar == Some(shared.glyphs[0].entry));
    anyhow::ensure!(
        glyph_cache.rasterizations() == before_cross_path,
        "scalar alias rerasterized an existing shaped atlas entry"
    );
    glyph_cache.set_shaping_byte_limit_for_test(1);
    let uncached = glyph_cache
        .shape_run_styled("WWW", glyph_cache::FontStyle::default(), queue)
        .ok_or_else(|| anyhow::anyhow!("over-budget run must still render"))?;
    anyhow::ensure!(uncached.glyphs.len() == 3);
    anyhow::ensure!(glyph_cache.shaping_cache_metrics().entries == 0);
    anyhow::ensure!(glyph_cache.shaping_cache_metrics().retained_bytes == 0);

    let mut text = TextPipeline::new(device, FORMAT, &glyph_cache)?;
    text.resize(queue, 64, 64);
    text.push_char('W', 8.0, 8.0, [1.0; 4]);
    text.upload(device, queue, &mut glyph_cache);
    render_offscreen(
        device,
        queue,
        FORMAT,
        (64, 64),
        wgpu::Color::BLACK,
        |pass| text.draw_layer(pass, 0),
    )?;
    Ok(())
}

#[cfg(test)]
fn queue_styled_test_run(
    text: &mut TextPipeline,
    content: &str,
    origin_x: f32,
    origin_y: f32,
    cell_width: f32,
    color: [f32; 4],
    style: glyph_cache::FontStyle,
    clip: TextClip,
) {
    let mut run = TextRunAccumulator::default();
    for (index, character) in content.chars().enumerate() {
        run.push(
            text,
            character,
            origin_x + index as f32 * cell_width,
            origin_y,
            color,
            style,
            clip,
            cell_width,
        );
    }
    run.flush(text);
}

#[cfg(test)]
pub(super) fn queue_test_run(
    text: &mut TextPipeline,
    content: &str,
    cell_width: f32,
    clip: TextClip,
) {
    queue_styled_test_run(
        text,
        content,
        12.0,
        12.0,
        cell_width,
        [0.92, 0.88, 0.78, 1.0],
        glyph_cache::FontStyle::default(),
        clip,
    );
}

#[cfg(test)]
fn render_text_snapshot(
    device: &Device,
    queue: &Queue,
    text: &mut TextPipeline,
    glyph_cache: &mut GlyphCache,
    content: &str,
) -> Result<Vec<u8>> {
    render_queued_text_snapshot(device, queue, text, glyph_cache, |text, cell_width| {
        queue_styled_test_run(
            text,
            content,
            12.0,
            12.0,
            cell_width,
            [0.92, 0.88, 0.78, 1.0],
            glyph_cache::FontStyle::default(),
            TextClip {
                left: 0.0,
                top: 0.0,
                right: 256.0,
                bottom: 64.0,
            },
        );
    })
}

#[cfg(test)]
fn render_queued_text_snapshot(
    device: &Device,
    queue: &Queue,
    text: &mut TextPipeline,
    glyph_cache: &mut GlyphCache,
    queue_content: impl FnOnce(&mut TextPipeline, f32),
) -> Result<Vec<u8>> {
    const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 64;
    text.clear();
    let (cell_width, _) = glyph_cache.cell_size();
    queue_content(text, cell_width);
    text.upload(device, queue, glyph_cache);

    render_offscreen(
        device,
        queue,
        FORMAT,
        (WIDTH, HEIGHT),
        wgpu::Color::BLACK,
        |pass| text.draw_layer(pass, 0),
    )
}

/// Produce a small, deterministic renderer snapshot for the GPU render
/// smoke's visual baseline. The bundled test font removes host font discovery
/// from the baseline while retaining Termeric's real atlas, shaping, upload,
/// shader, blending, and GPU readback paths.
#[cfg(test)]
pub(crate) fn headless_gui_visual_snapshot(
    device: &Device,
    queue: &Queue,
) -> Result<(Vec<u8>, u32, u32)> {
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 64;
    const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
    let mut glyph_cache = GlyphCache::for_test(device, queue, 16.0, false)?;
    glyph_cache.install_test_font(
        glyph_cache::deterministic_test_font_data(),
        "Fira Mono",
        queue,
    );
    let mut text = TextPipeline::new(device, FORMAT, &glyph_cache)?;
    text.resize(queue, WIDTH, HEIGHT);
    let pixels = render_text_snapshot(
        device,
        queue,
        &mut text,
        &mut glyph_cache,
        "Termeric GUI != -> 17",
    )?;
    Ok((pixels, WIDTH, HEIGHT))
}

/// Render deterministic enabled/disabled ligature snapshots through the real
/// atlas and GPU pipeline, and verify unchanged frames reuse shaped runs.
#[cfg(test)]
pub(crate) fn headless_ligature_render_smoke(device: &Device, queue: &Queue) -> Result<()> {
    const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
    let mut glyph_cache = GlyphCache::for_test(device, queue, 16.0, true)?;
    glyph_cache.install_test_font(
        glyph_cache::deterministic_test_font_data(),
        "Fira Mono",
        queue,
    );
    let mut text = TextPipeline::new(device, FORMAT, &glyph_cache)?;
    text.resize(queue, 256, 64);

    let enabled = render_text_snapshot(device, queue, &mut text, &mut glyph_cache, "ffi")?;
    let repeated = render_text_snapshot(device, queue, &mut text, &mut glyph_cache, "ffi")?;
    anyhow::ensure!(
        enabled == repeated,
        "unchanged ligature snapshot was unstable"
    );
    let warm_metrics = glyph_cache.shaping_cache_metrics();
    anyhow::ensure!(warm_metrics.misses == 1 && warm_metrics.hits >= 1);

    glyph_cache.set_ligatures(false);
    let disabled = render_text_snapshot(device, queue, &mut text, &mut glyph_cache, "ffi")?;
    anyhow::ensure!(
        enabled != disabled,
        "ligature setting did not change rendered pixels"
    );
    let toggled_metrics = glyph_cache.shaping_cache_metrics();
    anyhow::ensure!(toggled_metrics.misses == 2 && toggled_metrics.entries == 1);

    glyph_cache.set_ligatures(true);
    let explicit_disabled = ["liga", "clig", "calt", "dlig"]
        .into_iter()
        .map(|tag| (tag.to_string(), 0))
        .collect::<BTreeMap<_, _>>();
    anyhow::ensure!(glyph_cache.set_font_features(&explicit_disabled));
    anyhow::ensure!(
        glyph_cache.shaping_cache_metrics().entries == 0,
        "OpenType feature change retained stale shaping entries"
    );
    let overridden = render_text_snapshot(device, queue, &mut text, &mut glyph_cache, "ffi")?;
    anyhow::ensure!(
        overridden == disabled,
        "explicit OpenType values did not override the ligature preset"
    );
    anyhow::ensure!(!glyph_cache.set_font_features(&explicit_disabled));
    anyhow::ensure!(
        glyph_cache.shaping_cache_metrics().entries == 1,
        "assigning identical OpenType features invalidated a warm run"
    );
    glyph_cache.set_font_features(&BTreeMap::new());

    let mixed_snapshot =
        |text: &mut TextPipeline, glyph_cache: &mut GlyphCache| -> Result<Vec<u8>> {
            render_queued_text_snapshot(device, queue, text, glyph_cache, |text, cell_width| {
                let clip = TextClip {
                    left: 0.0,
                    top: 0.0,
                    right: 218.0,
                    bottom: 64.0,
                };
                queue_styled_test_run(
                    text,
                    "ffi",
                    12.0,
                    12.0,
                    cell_width,
                    [0.92, 0.88, 0.78, 1.0],
                    glyph_cache::FontStyle::default(),
                    clip,
                );
                queue_styled_test_run(
                    text,
                    "ffi",
                    84.0,
                    12.0,
                    cell_width,
                    [0.35, 0.78, 0.62, 1.0],
                    glyph_cache::FontStyle {
                        bold: true,
                        italic: false,
                    },
                    clip,
                );
                queue_styled_test_run(
                    text,
                    "ffi",
                    174.0,
                    12.0,
                    cell_width,
                    [0.78, 0.52, 0.86, 1.0],
                    glyph_cache::FontStyle {
                        bold: false,
                        italic: true,
                    },
                    clip,
                );
            })
        };

    glyph_cache.set_ligatures(true);
    let mixed_enabled = mixed_snapshot(&mut text, &mut glyph_cache)?;
    let mixed_repeated = mixed_snapshot(&mut text, &mut glyph_cache)?;
    anyhow::ensure!(
        mixed_enabled == mixed_repeated,
        "mixed-style ligature snapshot was unstable"
    );
    glyph_cache.set_ligatures(false);
    let mixed_disabled = mixed_snapshot(&mut text, &mut glyph_cache)?;
    anyhow::ensure!(
        mixed_enabled != mixed_disabled,
        "mixed-style ligature snapshot did not change when features were disabled"
    );

    let metrics_before_ranges = glyph_cache.cell_size();
    anyhow::ensure!(glyph_cache.shaping_cache_metrics().entries > 0);
    glyph_cache.set_font_families(
        "Fira Mono".into(),
        &[],
        &[crate::config::FontRangeOverride {
            start: u32::from('0'),
            end: u32::from('9'),
            family: "Fira Mono".into(),
        }],
        queue,
    );
    anyhow::ensure!(
        glyph_cache.shaping_cache_metrics().entries == 0,
        "font-range policy change retained stale shaping entries"
    );
    anyhow::ensure!(
        glyph_cache.cell_size() == metrics_before_ranges,
        "font-range policy changed primary cell metrics"
    );
    let ranged = render_text_snapshot(device, queue, &mut text, &mut glyph_cache, "012")?;
    anyhow::ensure!(
        ranged.iter().any(|pixel| *pixel != 0),
        "font-range scalar/run path produced no pixels"
    );
    Ok(())
}

/// Exercise the parser-facing SGR style model through real glyph and rectangle
/// pipelines. The assertions are a portable geometry golden: they verify face
/// requests, foreground suppression, decoration primitive counts, and pane
/// clipping without depending on platform-specific font pixels.
#[cfg(test)]
pub(crate) fn headless_sgr_render_smoke(device: &Device, queue: &Queue) -> Result<()> {
    const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 128;
    let theme = Theme::load_builtin("turmeric")?;
    let palette = TerminalPalette::from_theme(&theme);
    let dynamic_colors = termeric_core::DynamicColors::default();
    let mut glyph_cache = GlyphCache::for_test(device, queue, 14.0, false)?;
    let mut text = TextPipeline::new(device, FORMAT, &glyph_cache)?;
    let mut rects = RectPipeline::new(device, FORMAT)?;
    text.resize(queue, WIDTH, HEIGHT);
    rects.resize(queue, WIDTH, HEIGHT);
    let (_, cell_height) = glyph_cache.cell_size();
    let clip = TextClip {
        left: 0.0,
        top: 0.0,
        right: 96.0,
        bottom: 64.0,
    };

    let text_cases = [
        (CellAttrs::empty(), false, false),
        (CellAttrs::BOLD, true, false),
        (CellAttrs::ITALIC, false, true),
        (CellAttrs::BOLD | CellAttrs::ITALIC, true, true),
        (CellAttrs::DIM, false, false),
        (CellAttrs::INVERSE, false, false),
    ];
    for (index, (attrs, bold, italic)) in text_cases.iter().copied().enumerate() {
        let mut cell = termeric_core::Cell::with_char((b'A' + index as u8) as char);
        cell.attrs = attrs;
        let style = ResolvedCellStyle::resolve(
            &palette,
            &dynamic_colors,
            &cell,
            false,
            TextBlinkPhases::default(),
        );
        anyhow::ensure!(style.font.bold == bold && style.font.italic == italic);
        text.push_styled_char(
            (b'A' + index as u8) as char,
            8.0 + index as f32 * 13.0,
            8.0,
            style.text_color(None, 1.0),
            style.font,
            clip,
        );
    }

    for (attrs, phases) in [
        (CellAttrs::HIDDEN, TextBlinkPhases::default()),
        (
            CellAttrs::BLINK,
            TextBlinkPhases {
                slow_visible: false,
                rapid_visible: true,
            },
        ),
        (
            CellAttrs::RAPID_BLINK,
            TextBlinkPhases {
                slow_visible: true,
                rapid_visible: false,
            },
        ),
    ] {
        let mut cell = termeric_core::Cell::with_char('X');
        cell.attrs = attrs;
        let style = ResolvedCellStyle::resolve(&palette, &dynamic_colors, &cell, false, phases);
        anyhow::ensure!(
            !style.paint_foreground,
            "concealed SGR text would be painted"
        );
    }

    let queued = text.take_queued_since(0);
    anyhow::ensure!(queued.len() == text_cases.len());
    let text_batch = TextPipeline::build_batch(&queued, queue, &mut glyph_cache);
    anyhow::ensure!(!text_batch.vertices.is_empty());
    anyhow::ensure!(text_batch.vertices.iter().all(|vertex| {
        vertex.position[0] >= clip.left
            && vertex.position[0] <= clip.right
            && vertex.position[1] >= clip.top
            && vertex.position[1] <= clip.bottom
    }));

    let decoration_goldens = [
        (CellAttrs::UNDERLINE, 1_usize),
        (CellAttrs::DOUBLE_UNDERLINE, 2),
        (CellAttrs::CURLY_UNDERLINE, 8),
        (CellAttrs::DOTTED_UNDERLINE, 4),
        (CellAttrs::DASHED_UNDERLINE, 2),
        (CellAttrs::STRIKE, 1),
        (CellAttrs::UNDERLINE | CellAttrs::STRIKE, 2),
    ];
    for (attrs, expected_rectangles) in decoration_goldens {
        let checkpoint = rects.checkpoint();
        let mut cell = termeric_core::Cell::with_char('x');
        cell.attrs = attrs;
        let style = ResolvedCellStyle::resolve(
            &palette,
            &dynamic_colors,
            &cell,
            false,
            TextBlinkPhases::default(),
        );
        style::queue_sgr_decorations(&mut rects, style, 4.0, 48.0, 12.0, cell_height, 1.0, None);
        let batch = rects.take_since(checkpoint);
        anyhow::ensure!(
            batch.indices.len() / 6 == expected_rectangles,
            "SGR geometry golden changed for {attrs:?}"
        );
    }

    for (index, (attrs, _, _)) in text_cases.iter().copied().enumerate() {
        let mut cell = termeric_core::Cell::with_char((b'A' + index as u8) as char);
        cell.attrs = attrs;
        let style = ResolvedCellStyle::resolve(
            &palette,
            &dynamic_colors,
            &cell,
            false,
            TextBlinkPhases::default(),
        );
        text.push_styled_char(
            (b'A' + index as u8) as char,
            8.0 + index as f32 * 13.0,
            8.0,
            style.text_color(None, 1.0),
            style.font,
            clip,
        );
    }
    let mut decorated = termeric_core::Cell::with_char('x');
    decorated.attrs = CellAttrs::CURLY_UNDERLINE | CellAttrs::STRIKE;
    let decorated_style = ResolvedCellStyle::resolve(
        &palette,
        &dynamic_colors,
        &decorated,
        false,
        TextBlinkPhases::default(),
    );
    style::queue_sgr_decorations(
        &mut rects,
        decorated_style,
        8.0,
        48.0,
        80.0,
        cell_height,
        1.0,
        None,
    );
    text.upload(device, queue, &mut glyph_cache);
    rects.upload(device, queue);

    render_offscreen(
        device,
        queue,
        FORMAT,
        (WIDTH, HEIGHT),
        wgpu::Color::BLACK,
        |pass| {
            rects.draw_layer(pass, 0);
            text.draw_layer(pass, 0);
        },
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn headless_palette_input_smoke(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> anyhow::Result<()> {
    use super::{GlyphCache, TextPipeline, text::TextClip};
    use crate::ui::command_palette::PaletteLayout;
    let mut glyphs = GlyphCache::for_test(device, queue, 16.0, false)?;
    glyphs.install_test_font(
        super::glyph_cache::deterministic_test_font_data(),
        "Fira Mono",
        queue,
    );
    let mut text = TextPipeline::new(device, wgpu::TextureFormat::Rgba8Unorm, &glyphs)?;
    for width in [320.0, 800.0] {
        let layout = PaletteLayout::calculate(width, 600.0, 10.0, 24.0, 1.0, 0);
        for query in [
            "ASCII query ".repeat(20),
            "界e\u{301}👩\u{200d}💻 ASCII ".repeat(15),
        ] {
            let terminal = termeric_core::Terminal::new(80, 24);
            let mut search = crate::terminal::search::Search::new();
            search.start();
            assert!(search.set_query(&query, terminal.grid()));
            let info = search.build_info(query.len());
            let search_layout = crate::ui::search_bar::SearchBarLayout::calculate(
                &info,
                160.0,
                width.max(800.0),
                32.0,
                10.0,
                24.0,
            );
            for view in [
                layout.input_viewport(&query, query.len()).unwrap(),
                search_layout.input_viewport(&info, 10.0).unwrap(),
                crate::ui::shortcuts_panel::ShortcutsPanelLayout::calculate(
                    width, 600.0, 10.0, 24.0, 1.0, 0,
                )
                .input_viewport(&query, query.len(), 10.0, 24.0)
                .unwrap(),
                crate::ui::settings::SettingsModalLayout::calculate(
                    width, 600.0, 10.0, 24.0, 1.0, 0,
                )
                .input_viewport(&query, query.len(), 24.0)
                .unwrap(),
                crate::ui::session_panel::SessionPanelLayout::calculate(
                    width.max(800.0),
                    600.0,
                    0.0,
                    10.0,
                    24.0,
                    1.0,
                    true,
                    true,
                    0,
                )
                .rename_input_viewport(&query, query.len())
                .unwrap(),
            ] {
                let clip = TextClip {
                    left: view.text_x,
                    top: view.text_y,
                    right: view.right,
                    bottom: view.text_y + 24.0,
                };
                text.push_ui_run(
                    &query[view.text_range],
                    view.text_x,
                    view.text_y,
                    [1.0; 4],
                    10.0,
                    clip,
                );
                let queued = text.take_queued_since(0);
                anyhow::ensure!(queued.len() == 1 && queued[0].clip == Some(clip));
                let batch = TextPipeline::build_batch(&queued, queue, &mut glyphs);
                anyhow::ensure!(
                    !batch.vertices.is_empty(),
                    "query field produced no glyph geometry"
                );
                anyhow::ensure!(
                    batch.vertices.iter().all(|vertex| {
                        vertex.position[0] >= clip.left
                            && vertex.position[0] <= clip.right
                            && vertex.position[1] >= clip.top
                            && vertex.position[1] <= clip.bottom
                    }),
                    "query glyph escaped its input field"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod headless_image_readback_tests {
    use super::*;

    #[test]
    fn verifier_rejects_a_row_at_a_time_placeholder_frame() {
        let mut pixels = expected_headless_image_state(0, HEADLESS_IMAGE_ROWS);
        let half_width = HEADLESS_IMAGE_WIDTH / 2;
        for y in HEADLESS_IMAGE_CELL_HEIGHT..HEADLESS_IMAGE_HEIGHT {
            for x in half_width..HEADLESS_IMAGE_WIDTH {
                let offset = ((y * HEADLESS_IMAGE_WIDTH + x) * 4) as usize;
                pixels[offset..offset + 3].fill(0);
            }
        }

        let error =
            verify_headless_image_state(&pixels, 0, HEADLESS_IMAGE_ROWS, "row-at-a-time fixture")
                .unwrap_err();
        assert!(error.to_string().contains("Unicode-placeholder row 1"));
    }

    #[test]
    fn verifier_rejects_pixels_below_the_scrolled_footprint() {
        let visible_rows = HEADLESS_IMAGE_ROWS - 3;
        let mut pixels = expected_headless_image_state(3, visible_rows);
        let first_hidden_y = visible_rows * HEADLESS_IMAGE_CELL_HEIGHT;
        let offset = (first_hidden_y * HEADLESS_IMAGE_WIDTH * 4) as usize;
        pixels[offset] = 1;

        let error =
            verify_headless_image_state(&pixels, 3, visible_rows, "scrolled fixture").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("painted outside the expected scrolled image footprint")
        );
    }
}
