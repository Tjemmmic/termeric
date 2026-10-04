//! Font system setup, face attributes and features, configured family
//! resolution, and cell metrics.

use super::*;

pub(super) fn font_has_weight_axis(
    font_system: &mut FontSystem,
    font_id: cosmic_text::fontdb::ID,
    requested_weight: Weight,
) -> bool {
    font_system
        .get_font(font_id, requested_weight)
        .is_some_and(|font| {
            font.as_swash()
                .variations()
                .any(|axis| axis.tag().to_be_bytes() == *b"wght")
        })
}

pub(super) fn should_synthesize_bold(
    style: FontStyle,
    requested_weight: Weight,
    selected_face_is_light: bool,
    has_weight_axis: bool,
) -> bool {
    // Cosmic Text applies a `wght` coordinate while rasterizing variable
    // fonts. Applying our static-face embolden fallback too would make
    // variable bold text artificially heavier than requested.
    style.bold
        && requested_weight.0 >= Weight::SEMIBOLD.0
        && selected_face_is_light
        && !has_weight_axis
}

/// Load installed fonts while keeping fontconfig's generic `sans-serif`
/// alias, so chrome text uses the desktop's UI face. `FontSystem::new` would
/// replace that alias with a fixed family; its monospace and serif defaults
/// are kept so terminal text resolves exactly as before.
pub(super) fn desktop_font_system() -> FontSystem {
    let mut database = cosmic_text::fontdb::Database::new();
    database.load_system_fonts();
    database.set_monospace_family("Noto Sans Mono");
    database.set_serif_family("DejaVu Serif");
    let locale = sys_locale::get_locale().unwrap_or_else(|| {
        log::warn!("failed to get system locale, falling back to en-US");
        String::from("en-US")
    });
    log::info!(
        "UI font family: {}",
        database.family_name(&cosmic_text::fontdb::Family::SansSerif)
    );
    FontSystem::new_with_locale_and_db(locale, database)
}

pub(super) fn ui_face_attrs(face: UiFace) -> Attrs<'static> {
    let mut attrs = Attrs::new().family(if face.mono {
        Family::Monospace
    } else {
        Family::SansSerif
    });
    if face.bold {
        attrs = attrs.weight(Weight::BOLD);
    }
    if face.italic {
        attrs = attrs.style(Style::Italic);
    }
    attrs
}

pub(super) fn ui_text_metrics(size_px: f32) -> Metrics {
    Metrics::new(size_px, (size_px * 1.25).ceil())
}

pub(super) fn font_attrs(
    family: Family<'_>,
    style: FontStyle,
    font_weight: Weight,
    bold_font_weight: Weight,
) -> Attrs<'_> {
    Attrs::new()
        .family(family)
        .weight(if style.bold {
            bold_font_weight
        } else {
            font_weight
        })
        .style(if style.italic {
            Style::Italic
        } else {
            Style::Normal
        })
}

pub(super) fn terminal_font_features(ligatures: bool, overrides: &FontFeatures) -> FontFeatures {
    let mut features = FontFeatures::new();
    for feature in [
        FeatureTag::STANDARD_LIGATURES,
        FeatureTag::CONTEXTUAL_LIGATURES,
        FeatureTag::CONTEXTUAL_ALTERNATES,
        FeatureTag::DISCRETIONARY_LIGATURES,
    ] {
        if ligatures {
            features.enable(feature);
        } else {
            features.disable(feature);
        }
    }
    // Terminal advances are a fixed grid; proportional pair kerning must not
    // shift later cell origins.
    features.disable(FeatureTag::KERNING);
    features.features.extend_from_slice(&overrides.features);
    features
}

pub(super) fn compile_font_feature_overrides(overrides: &BTreeMap<String, u32>) -> FontFeatures {
    let mut features = FontFeatures::new();
    for (tag, value) in overrides {
        let bytes = tag.as_bytes();
        debug_assert_eq!(bytes.len(), 4, "font feature tags are validated by Config");
        if let [first, second, third, fourth] = bytes {
            features.set(FeatureTag::new(&[*first, *second, *third, *fourth]), *value);
        }
    }
    features
}

pub(super) fn collect_configured_families(
    font_system: &FontSystem,
    primary: &str,
    fallbacks: &[String],
) -> Vec<ConfiguredFamily> {
    let mut seen = HashSet::new();
    std::iter::once(primary)
        .chain(fallbacks.iter().map(String::as_str))
        .filter_map(|name| {
            let name = name.trim();
            if name.is_empty() || !seen.insert(name.to_ascii_lowercase()) {
                return None;
            }
            Some(ConfiguredFamily {
                name: name.to_string(),
                faces: configured_family_faces(font_system, name),
            })
        })
        .collect()
}

pub(super) fn collect_configured_range_overrides(
    font_system: &FontSystem,
    rules: &[FontRangeOverride],
) -> Vec<ConfiguredRangeOverride> {
    let mut configured = rules
        .iter()
        .map(|rule| ConfiguredRangeOverride {
            start: rule.start,
            end: rule.end,
            family: ConfiguredFamily {
                name: rule.family.clone(),
                faces: configured_family_faces(font_system, &rule.family),
            },
        })
        .collect::<Vec<_>>();
    configured.sort_unstable_by_key(|rule| (rule.start, rule.end));
    configured
}

/// Whole-pixel terminal cell geometry for the primary face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CellMetrics {
    pub(super) width: f32,
    pub(super) height: f32,
    /// Distance from the cell top to the glyph baseline. It comes from the
    /// face's ascent and descent centred in the line height, as cosmic-text
    /// lays out lines, and is whole so every row's baseline is pixel-aligned.
    pub(super) baseline: f32,
    pub(super) use_monospace_fallback: bool,
}

/// Measure cell dimensions for monospace grid
///
/// For terminal rendering, we need a consistent cell width. We measure the
/// advance width of a single character with basic shaping. Using 'M' as it's
/// typically representative of monospace font width.
pub(super) fn measure_cell(
    font_system: &mut FontSystem,
    metrics: &Metrics,
    font_family: &str,
    font_weight: Weight,
) -> CellMetrics {
    let family = family_for(font_family);
    let attrs = Attrs::new().family(family).weight(font_weight);

    // Measure a single 'M' character to get the natural width
    let mut buffer = Buffer::new(font_system, *metrics);
    buffer.set_text("M", &attrs, Shaping::Basic, None);
    buffer.shape_until_scroll(font_system, false);

    let mut measured_width: f32 = 0.0;
    let mut font_id: Option<cosmic_text::fontdb::ID> = None;
    let mut baseline: Option<f32> = None;
    if let Some(run) = buffer.layout_runs().next() {
        baseline = Some(run.line_y);
        if let Some(glyph) = run.glyphs.first() {
            measured_width = glyph.w;
            font_id = Some(glyph.font_id);
        }
    }

    // Try to get the actual font name and check if it's monospace
    let (actual_font_name, is_monospace) = font_id
        .map(|id| {
            let name = font_system
                .db()
                .face(id)
                .map(|face| {
                    face.families
                        .first()
                        .map(|(name, _)| name.clone())
                        .unwrap_or_else(|| "unknown".to_string())
                })
                .unwrap_or_else(|| "unknown".to_string());
            let is_mono = font_system.is_monospace(id);
            (name, is_mono)
        })
        .unwrap_or_else(|| ("none".to_string(), false));

    // Check if the requested font was found by comparing names
    let requested_lower = font_family.to_lowercase();
    let actual_lower = actual_font_name.to_lowercase();

    // Font matches if:
    // 1. User requested generic "monospace" - always matches
    // 2. Actual font name contains or is contained in requested name
    let is_generic_request = matches!(requested_lower.as_str(), "monospace" | "mono" | "default");
    let name_matches =
        actual_lower.contains(&requested_lower) || requested_lower.contains(&actual_lower);
    let font_was_found = is_generic_request || name_matches;

    // Only use fallback if font wasn't found AND got a non-monospace fallback
    // If the font WAS found (name matches), trust it even if is_monospace reports false
    let use_fallback = !font_was_found && !is_monospace;

    // Standard monospace fonts have width/height ratio around 0.5-0.6
    // If measured width is too large (> 0.75 * font_size), use heuristic for cell spacing
    let expected_max = metrics.font_size * 0.75;
    let width = if measured_width > 0.0 && measured_width <= expected_max {
        // Width looks reasonable, use it
        measured_width.round().max(1.0)
    } else if use_fallback {
        // Font not found and got proportional fallback - measure system monospace
        let mut mono_buffer = Buffer::new(font_system, *metrics);
        mono_buffer.set_text(
            "M",
            &Attrs::new().family(Family::Monospace).weight(font_weight),
            Shaping::Basic,
            None,
        );
        mono_buffer.shape_until_scroll(font_system, false);

        let mut mono_width: f32 = 0.0;
        if let Some(run) = mono_buffer.layout_runs().next() {
            baseline = Some(run.line_y);
            if let Some(glyph) = run.glyphs.first() {
                mono_width = glyph.w;
            }
        }

        log::warn!(
            "Font '{}' not found (got '{}'), using system monospace for rendering",
            font_family,
            actual_font_name
        );

        if mono_width > 0.0 {
            mono_width.round().max(1.0)
        } else {
            (metrics.font_size * 0.6).round().max(1.0)
        }
    } else {
        // Font was found but width seems off - still use measured width, just log it
        log::debug!(
            "Font '{}' measured width {:.2} exceeds expected max {:.2}, but using it anyway",
            font_family,
            measured_width,
            expected_max
        );
        measured_width.round().max(1.0)
    };

    let height = metrics.line_height.round().max(1.0);
    let baseline = baseline.unwrap_or(height * 0.8).round().clamp(1.0, height);

    log::info!(
        "Cell size for '{}' (actual='{}', found={}, fallback={}): {}x{}, baseline {} (measured_w={:.2})",
        font_family,
        actual_font_name,
        font_was_found,
        use_fallback,
        width,
        height,
        baseline,
        measured_width
    );

    CellMetrics {
        width,
        height,
        baseline,
        use_monospace_fallback: use_fallback,
    }
}

pub(super) fn family_for(font_family: &str) -> Family<'_> {
    let trimmed = font_family.trim();
    if trimmed.is_empty() {
        return Family::Monospace;
    }

    let lower = trimmed.to_ascii_lowercase();
    if matches!(lower.as_str(), "monospace" | "mono" | "default") {
        Family::Monospace
    } else {
        Family::Name(trimmed)
    }
}

pub(super) fn resolve_configured_family(
    font_system: &FontSystem,
    primary: &str,
    fallbacks: &[String],
) -> String {
    let is_generic = |name: &str| {
        matches!(
            name.trim().to_ascii_lowercase().as_str(),
            "monospace" | "mono" | "default"
        )
    };
    let installed = |name: &str| {
        font_system.db().faces().any(|face| {
            face.families
                .iter()
                .any(|(family, _)| family.eq_ignore_ascii_case(name.trim()))
        })
    };

    std::iter::once(primary)
        .chain(fallbacks.iter().map(String::as_str))
        .find(|name| is_generic(name) || installed(name))
        .unwrap_or("monospace")
        .to_string()
}
