//! Which configured or platform face covers a character, and the font
//! coverage report built from the same rules.

use super::*;

pub(super) fn configured_family_faces(
    font_system: &FontSystem,
    family: &str,
) -> Vec<(cosmic_text::fontdb::ID, Weight)> {
    let requested = family_for(family);
    match requested {
        Family::Name(name) => font_system
            .db()
            .faces()
            .filter(|face| {
                face.families
                    .iter()
                    .any(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            })
            .map(|face| (face.id, face.weight))
            .collect(),
        generic => {
            let families = [generic];
            font_system
                .db()
                .query(&cosmic_text::fontdb::Query {
                    families: &families,
                    weight: Weight::NORMAL,
                    stretch: cosmic_text::fontdb::Stretch::Normal,
                    style: Style::Normal,
                })
                .and_then(|id| font_system.db().face(id).map(|face| (id, face.weight)))
                .into_iter()
                .collect()
        }
    }
}

pub(super) fn first_configured_family_with_coverage(
    font_system: &mut FontSystem,
    families: &[ConfiguredFamily],
    text: &str,
) -> Option<usize> {
    if !text
        .chars()
        .any(|character| !is_font_default_ignorable(character))
    {
        return (!families.is_empty()).then_some(0);
    }
    families
        .iter()
        .position(|family| configured_family_covers(font_system, family, text))
}

pub(super) fn configured_family_covers(
    font_system: &mut FontSystem,
    family: &ConfiguredFamily,
    text: &str,
) -> bool {
    family.faces.iter().any(|&(id, weight)| {
        font_system.get_font(id, weight).is_some_and(|font| {
            let charmap = font.as_swash().charmap();
            text.chars()
                .filter(|character| !is_font_default_ignorable(*character))
                .all(|character| charmap.map(character) != 0)
        })
    })
}

pub(super) fn range_override_family_with_coverage<'a>(
    font_system: &mut FontSystem,
    rules: &'a [ConfiguredRangeOverride],
    grapheme: &str,
) -> Option<&'a str> {
    let rule = matching_range_override(rules, grapheme)?;
    configured_family_covers(font_system, &rule.family, grapheme)
        .then_some(rule.family.name.as_str())
}

pub(super) fn matching_range_override<'a>(
    rules: &'a [ConfiguredRangeOverride],
    grapheme: &str,
) -> Option<&'a ConfiguredRangeOverride> {
    let codepoint = grapheme
        .chars()
        .find(|character| !is_font_default_ignorable(*character))? as u32;
    let candidate = rules.partition_point(|rule| rule.start <= codepoint);
    let rule = rules.get(candidate.checked_sub(1)?)?;
    if codepoint > rule.end {
        return None;
    }
    Some(rule)
}

pub(super) fn diagnose_font_coverage(
    font_system: &mut FontSystem,
    metrics: Metrics,
    font_weight: Weight,
    configured_primary: &str,
    fallback_families: &[String],
    range_overrides: &[ConfiguredRangeOverride],
    default_family: &str,
    text: &str,
) -> std::result::Result<FontCoverageReport, String> {
    if text.is_empty() {
        return Err("enter literal text or an exact Unicode scalar".into());
    }
    if text.len() > MAX_FONT_COVERAGE_INPUT_BYTES {
        return Err(format!(
            "coverage input must be no more than {MAX_FONT_COVERAGE_INPUT_BYTES} UTF-8 bytes"
        ));
    }
    if text.chars().any(char::is_control) {
        return Err("coverage input must not contain control characters".into());
    }

    let mut seen = HashSet::new();
    let configured = std::iter::once((FontCoverageSource::Primary, configured_primary.to_string()))
        .chain(
            fallback_families
                .iter()
                .take(MAX_FONT_FALLBACKS)
                .enumerate()
                .map(|(index, family)| {
                    (
                        FontCoverageSource::Fallback { index: index + 1 },
                        family.clone(),
                    )
                }),
        )
        .filter(|(_, family)| seen.insert(family.trim().to_ascii_lowercase()))
        .map(|(source, family)| {
            let configured = ConfiguredFamily {
                faces: configured_family_faces(font_system, &family),
                name: family,
            };
            (source, configured)
        })
        .collect::<Vec<_>>();

    let mut entries = Vec::new();
    let mut graphemes_truncated = false;
    for grapheme in text.graphemes(true) {
        if entries.len() == MAX_FONT_COVERAGE_GRAPHEMES {
            graphemes_truncated = true;
            break;
        }
        let codepoints = grapheme.chars().map(u32::from).collect::<Vec<_>>();
        let substantive = grapheme
            .chars()
            .any(|character| !is_font_default_ignorable(character));
        if !substantive {
            entries.push(FontCoverageEntry {
                grapheme: grapheme.into(),
                codepoints,
                attempts: Vec::new(),
                route: FontCoverageRoute::DefaultIgnorable {
                    family: default_family.into(),
                },
            });
            continue;
        }

        let mut attempts = Vec::new();
        let mut route = None;
        if let Some(rule) = matching_range_override(range_overrides, grapheme) {
            let complete = configured_family_covers(font_system, &rule.family, grapheme);
            let source = FontCoverageSource::Range {
                start: rule.start,
                end: rule.end,
            };
            attempts.push(FontCoverageAttempt {
                source: source.clone(),
                family: rule.family.name.clone(),
                installed: !rule.family.faces.is_empty(),
                complete,
            });
            if complete {
                route = Some(FontCoverageRoute::Explicit {
                    source,
                    family: rule.family.name.clone(),
                });
            }
        }
        if route.is_none() {
            for (source, family) in &configured {
                let complete = configured_family_covers(font_system, family, grapheme);
                attempts.push(FontCoverageAttempt {
                    source: source.clone(),
                    family: family.name.clone(),
                    installed: !family.faces.is_empty(),
                    complete,
                });
                if complete {
                    route = Some(FontCoverageRoute::Explicit {
                        source: source.clone(),
                        family: family.name.clone(),
                    });
                    break;
                }
            }
        }
        let route = route.unwrap_or_else(|| {
            platform_fallback_route(font_system, metrics, font_weight, default_family, grapheme)
        });
        entries.push(FontCoverageEntry {
            grapheme: grapheme.into(),
            codepoints,
            attempts,
            route,
        });
    }

    Ok(FontCoverageReport {
        entries,
        graphemes_truncated,
    })
}

/// The glyph that draws `c` in the family `attrs` name: with basic shaping
/// from that family alone, with advanced shaping also from the platform's
/// fallback fonts for its script. `None` when no font has one.
pub(super) fn char_glyph(
    font_system: &mut FontSystem,
    metrics: Metrics,
    c: char,
    attrs: &Attrs<'_>,
    shaping: Shaping,
) -> Option<(CacheKey, cosmic_text::fontdb::ID)> {
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_text(&c.to_string(), attrs, shaping, None);
    buffer.shape_until_scroll(font_system, false);
    buffer
        .layout_runs()
        .flat_map(|run| run.glyphs.iter())
        .find(|glyph| glyph.glyph_id != 0)
        .map(|glyph| (glyph.physical((0.0, 0.0), 1.0).cache_key, glyph.font_id))
}

pub(super) fn platform_fallback_route(
    font_system: &mut FontSystem,
    metrics: Metrics,
    font_weight: Weight,
    default_family: &str,
    grapheme: &str,
) -> FontCoverageRoute {
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_text(
        grapheme,
        &font_attrs(
            family_for(default_family),
            FontStyle::default(),
            font_weight,
            font_weight,
        ),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);
    let glyphs = buffer
        .layout_runs()
        .flat_map(|run| run.glyphs.iter())
        .map(|glyph| {
            let substantive_cluster = grapheme
                .get(glyph.start..glyph.end)
                .unwrap_or(grapheme)
                .chars()
                .any(|character| !is_font_default_ignorable(character));
            (glyph.font_id, glyph.glyph_id, substantive_cluster)
        })
        .collect::<Vec<_>>();
    if glyphs.is_empty()
        || glyphs
            .iter()
            .any(|(_, glyph_id, substantive)| *substantive && *glyph_id == 0)
        || !glyphs.iter().any(|(_, glyph_id, _)| *glyph_id != 0)
    {
        return FontCoverageRoute::Missing;
    }

    let mut seen = HashSet::new();
    let mut families = Vec::new();
    let mut names_truncated = false;
    for (font_id, glyph_id, _) in glyphs {
        if glyph_id == 0 {
            continue;
        }
        let name = font_system
            .db()
            .face(font_id)
            .and_then(|face| face.families.first())
            .map(|(family, _)| family.clone())
            .unwrap_or_else(|| format!("fontdb {:?}", font_id));
        let (name, name_truncated) = bounded_font_coverage_family(&name);
        names_truncated |= name_truncated;
        if !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        if families.len() == MAX_FONT_COVERAGE_PLATFORM_FACES {
            names_truncated = true;
            break;
        }
        families.push(name);
    }
    if families.is_empty() {
        FontCoverageRoute::Missing
    } else {
        FontCoverageRoute::Platform {
            families,
            names_truncated,
        }
    }
}

pub(super) fn bounded_font_coverage_family(name: &str) -> (String, bool) {
    (
        crate::safe_text::truncate_with_ellipsis(name, MAX_FONT_COVERAGE_FAMILY_CHARS),
        name.chars().nth(MAX_FONT_COVERAGE_FAMILY_CHARS).is_some(),
    )
}

pub(super) fn configured_family_spans(
    font_system: &mut FontSystem,
    families: &[ConfiguredFamily],
    range_overrides: &[ConfiguredRangeOverride],
    text: &str,
    default_family: &str,
) -> Vec<(Range<usize>, String)> {
    coalesced_family_spans(text, |grapheme| {
        range_override_family_with_coverage(font_system, range_overrides, grapheme)
            .or_else(|| {
                first_configured_family_with_coverage(font_system, families, grapheme)
                    .map(|index| families[index].name.as_str())
            })
            .unwrap_or(default_family)
    })
}

/// Family/range policy is immutable during one call. Reuse only the previous
/// complete grapheme's resolution, and allocate a name only when a new span
/// starts. No borrowed text or family name survives this call.
pub(super) fn coalesced_family_spans<'a>(
    text: &str,
    mut resolve: impl FnMut(&str) -> &'a str,
) -> Vec<(Range<usize>, String)> {
    let mut spans: Vec<(Range<usize>, String)> = Vec::new();
    let mut previous_grapheme = None;
    for (start, grapheme) in text.grapheme_indices(true) {
        let end = start + grapheme.len();
        let family = match previous_grapheme {
            Some((previous, family)) if previous == grapheme => family,
            _ => resolve(grapheme),
        };
        previous_grapheme = Some((grapheme, family));
        if let Some((range, previous)) = spans.last_mut()
            && previous == family
        {
            range.end = end;
        } else {
            spans.push((start..end, family.to_owned()));
        }
    }
    spans
}

/// Match the shaping engine's default-ignorable policy. Some Unicode
/// default-ignorable codepoints are intentionally excluded because HarfBuzz
/// preserves them as spacing glyphs or shaping controls.
pub(super) fn is_font_default_ignorable(character: char) -> bool {
    matches!(character, '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{FEFF}')
        || ('\u{17B4}'..='\u{17B5}').contains(&character)
        || ('\u{180B}'..='\u{180E}').contains(&character)
        || ('\u{200B}'..='\u{200F}').contains(&character)
        || ('\u{202A}'..='\u{202E}').contains(&character)
        || ('\u{2060}'..='\u{206F}').contains(&character)
        || ('\u{FE00}'..='\u{FE0F}').contains(&character)
        || ('\u{FFF0}'..='\u{FFF8}').contains(&character)
        || ('\u{1D173}'..='\u{1D17A}').contains(&character)
        || ('\u{E0000}'..='\u{E0FFF}').contains(&character)
}
