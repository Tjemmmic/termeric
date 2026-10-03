//! Command palette rendering — centered modal with search and results list

use super::Renderer;
use crate::ui::color::scale_alpha;
use crate::ui::command_palette::{PaletteInfo, PaletteLayout};

#[cfg(test)]
pub(crate) fn headless_palette_input_smoke(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> anyhow::Result<()> {
    use super::{GlyphCache, TextPipeline, text::TextClip};
    let mut glyphs = GlyphCache::new(
        device,
        queue,
        16.0,
        "monospace",
        &[],
        &[],
        400,
        700,
        1.2,
        false,
    )?;
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
                crate::session::panel::SessionPanelLayout::calculate(
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
                let batch = TextPipeline::build_batch(queued, queue, &mut glyphs);
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

impl Renderer {
    /// A centred modal's soft shadow, hairline border, and raised surface,
    /// faded in with `animation`. Shared by the palette-style dialogs.
    pub(super) fn build_modal_frame(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        animation: f32,
    ) {
        let px = |logical: f32| (logical * self.scale_factor).round().max(1.0);
        let fade = |color: [f32; 4]| scale_alpha(color, animation);
        let radius = px(10.0);
        for (spread, alpha) in [(px(2.0), 0.18), (px(10.0), 0.10)] {
            self.rect_pipeline.push_rounded_rect(
                x - spread,
                y - spread / 2.0 + px(3.0),
                width + spread * 2.0,
                height + spread * 2.0,
                [0.0, 0.0, 0.0, alpha * animation],
                radius + spread,
            );
        }
        let line = px(1.0);
        self.rect_pipeline.push_rounded_rect(
            x,
            y,
            width,
            height,
            fade(self.ui.menu_border),
            radius,
        );
        self.rect_pipeline.push_rounded_rect(
            x + line,
            y + line,
            width - line * 2.0,
            height - line * 2.0,
            fade(self.ui.menu_bg),
            radius - line,
        );
    }

    /// Build command palette render data — centered modal with a light scrim.
    /// The query field keeps the shared `PaletteLayout` geometry that caret,
    /// IME, pointer, and accessibility code use; rows draw in the UI face.
    pub(super) fn build_command_palette(
        &mut self,
        info: &PaletteInfo,
        window_width: f32,
        window_height: f32,
    ) {
        let anim = info.animation;
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let scale = self.scale_factor;
        let px = |logical: f32| (logical * scale).round().max(1.0);
        let fade = |color: [f32; 4], alpha: f32| scale_alpha(color, alpha * anim);

        let scrim = fade(self.ui.palette_scrim, 1.0);
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, window_height, scrim);

        let layout = PaletteLayout::calculate(
            window_width,
            window_height,
            cell_width,
            cell_height,
            anim,
            info.entries.len(),
        );
        let (panel_x, panel_y) = (layout.panel_x.round(), layout.panel_y.round());
        let (panel_width, panel_height) = (layout.panel_width.round(), layout.panel_height.round());
        let padding = layout.padding;
        let row_height = layout.row_height;

        self.build_modal_frame(panel_x, panel_y, panel_width, panel_height, anim);
        let line = px(1.0);

        // ── Query field ──
        let input_x = panel_x + padding;
        let input_y = panel_y + padding;
        let input_w = panel_width - padding * 2.0;
        let input_height = layout.input_height;
        self.rect_pipeline.push_rounded_rect(
            input_x,
            input_y,
            input_w,
            input_height,
            fade(self.ui.palette_input_bg, 0.8),
            px(6.0),
        );
        // Magnifier in the slot before the query text.
        self.push_magnifier(
            input_x + cell_width * crate::ui::constants::FILTER_ICON_CENTER_CELLS,
            input_y + input_height / 2.0,
            fade(self.ui.palette_text_dim, 1.0),
            fade(self.ui.palette_input_bg, 0.8),
        );

        // One grapheme-safe, caret-following viewport also anchors native IME.
        if let Some(view) = layout.input_viewport(&info.input, info.cursor) {
            let clip = super::text::TextClip {
                left: view.text_x,
                top: input_y,
                right: view.right,
                bottom: input_y + input_height,
            };
            if info.input.is_empty() {
                if let Some(hint) = layout.input_viewport(&info.placeholder, 0) {
                    self.text_pipeline.push_ui_run(
                        &info.placeholder[hint.text_range],
                        view.text_x,
                        view.text_y,
                        fade(self.ui.palette_text_dim, 0.7),
                        cell_width,
                        clip,
                    );
                }
            } else {
                self.text_pipeline.push_ui_run(
                    &info.input[view.text_range],
                    view.text_x,
                    view.text_y,
                    fade(self.ui.palette_text, 1.0),
                    cell_width,
                    clip,
                );
            }
            self.rect_pipeline.push_rect(
                view.cursor_x,
                view.text_y,
                px(2.0),
                cell_height,
                fade(self.ui.palette_accent, 0.9),
            );
        }

        // ── Separator ──
        let sep_y = input_y + input_height + layout.gap;
        self.rect_pipeline.push_rect(
            panel_x + line,
            sep_y.round(),
            panel_width - line * 2.0,
            layout.separator_height,
            fade(self.ui.menu_border, 1.0),
        );

        // ── Results ──
        // `info.entries` is already sliced to the visible window.
        let size = self.ui_label_px();
        let list_y = layout.list_y;
        let text_x = panel_x + padding + cell_width * 0.5;
        let scrollbar_width = px(3.0);
        let right = panel_x + panel_width - padding - scrollbar_width - px(6.0);
        for (row_index, entry) in info.entries.iter().enumerate().take(layout.visible_count) {
            let row_y = (list_y + row_index as f32 * row_height).round();
            let selected = row_index == info.selected;
            if selected {
                self.rect_pipeline.push_rounded_rect(
                    panel_x + padding * 0.5,
                    row_y,
                    panel_width - padding,
                    row_height,
                    fade(self.ui.menu_highlight, 1.0),
                    px(6.0),
                );
            }
            let text = fade(self.ui.palette_text, if selected { 1.0 } else { 0.88 });
            let dim = fade(self.ui.menu_text_dim, 1.0);
            let detail = entry
                .shortcut
                .clone()
                .or_else(|| info.show_category.then(|| entry.category.clone()))
                .filter(|detail| !detail.is_empty());
            let detail_width = match detail {
                Some(detail) => {
                    let width = self.ui_text_width(&detail, size);
                    let available = (right - text_x) * 0.5;
                    self.push_ui_label(
                        &detail,
                        (right - width).max(right - available),
                        row_y,
                        row_height,
                        available,
                        size,
                        dim,
                    );
                    width.min(available) + px(16.0)
                }
                None => 0.0,
            };
            self.push_ui_label_highlighted(
                &entry.title,
                &entry.match_positions,
                text_x,
                row_y,
                row_height,
                right - detail_width - text_x,
                size,
                text,
                fade(self.ui.palette_match_highlight, 1.0),
            );
        }

        // ── Scrollbar: a thin thumb only while results overflow ──
        let visible_count = layout.visible_count;
        if info.total_entries > visible_count && visible_count > 0 {
            let track_x = panel_x + panel_width - padding * 0.5 - scrollbar_width;
            let track_h = layout.list_height;
            let thumb_h =
                (track_h * visible_count as f32 / info.total_entries as f32).max(px(24.0));
            let max_scroll = info.total_entries.saturating_sub(visible_count);
            let fraction = if max_scroll > 0 {
                info.scroll_offset as f32 / max_scroll as f32
            } else {
                0.0
            };
            self.rect_pipeline.push_rounded_rect(
                track_x,
                list_y + fraction * (track_h - thumb_h),
                scrollbar_width,
                thumb_h,
                fade(self.ui.menu_text_dim, 0.6),
                scrollbar_width / 2.0,
            );
        }

        // ── Footer: result count, or pending/error status ──
        let footer_y = (panel_y + panel_height - layout.footer_height - padding * 0.5).round();
        self.rect_pipeline.push_rect(
            panel_x + line,
            footer_y,
            panel_width - line * 2.0,
            line,
            fade(self.ui.menu_border, 1.0),
        );
        let noun = |count: usize| {
            if count == 1 {
                info.item_noun.clone()
            } else {
                format!("{}s", info.item_noun)
            }
        };
        let footer_text = if let Some(status) = info.status {
            status.to_string()
        } else if info.total_entries < info.total_count {
            format!(
                "{} of {} {}",
                info.total_entries,
                info.total_count,
                noun(info.total_count)
            )
        } else {
            format!("{} {}", info.total_count, noun(info.total_count))
        };
        self.push_ui_label(
            &footer_text,
            panel_x + padding,
            footer_y,
            layout.footer_height + padding * 0.5,
            panel_width - padding * 2.0,
            size * 0.9,
            fade(self.ui.menu_text_dim, 1.0),
        );
    }
}
