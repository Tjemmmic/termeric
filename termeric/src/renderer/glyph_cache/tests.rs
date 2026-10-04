use super::caches::{MAX_UI_LAYOUT_BYTES, MAX_UI_LAYOUTS};
use super::coverage::{coalesced_family_spans, is_font_default_ignorable};

#[test]
fn ui_layouts_keep_what_is_used_and_stay_within_their_bounds() {
    let mut cache = UiLayoutCache::default();
    let layout = Arc::new(TextLayout {
        lines: Vec::new(),
        width: 0.0,
        height: 0.0,
    });
    fn spans(text: &str) -> [TextSpan<'_>; 1] {
        [TextSpan {
            text,
            face: UiFace::SANS,
        }]
    }
    let lookup = |cache: &mut UiLayoutCache, text: &str| cache.get(&spans(text), (1, 1), None);
    let insert = |cache: &mut UiLayoutCache, text: &str| {
        cache.insert(&spans(text), (1, 1), None, Arc::clone(&layout));
    };
    let on_screen = "on screen";
    insert(&mut cache, on_screen);
    // Many small stale paragraphs, then a few large ones, as a streamed
    // answer leaves behind; the paragraph on screen is used every frame.
    let small = (0..MAX_UI_LAYOUTS * 2).map(|index| index.to_string());
    let large = (0..20).map(|index| format!("{index}{}", "x".repeat(512 * 1024)));
    for text in small.chain(large) {
        assert!(lookup(&mut cache, on_screen).is_some());
        insert(&mut cache, &text);
        assert!(cache.count <= MAX_UI_LAYOUTS);
        assert!(cache.bytes <= MAX_UI_LAYOUT_BYTES);
    }
    assert!(lookup(&mut cache, on_screen).is_some());
    let counted: usize = cache.entries.values().map(Vec::len).sum();
    assert_eq!(counted, cache.count);
    assert!(lookup(&mut cache, "never laid out").is_none());
}

use super::*;
use std::sync::Arc;

fn cache_fixture(text: &str, count: usize) -> (LigatureKey, LigatureRun) {
    let key = LigatureKey::new(
        text,
        16.0,
        FontStyle::default(),
        GlyphPlacement::CellGrid,
        TextFace::Terminal,
    );
    let run = LigatureRun {
        glyphs: vec![
            ShapedGlyph {
                entry: GlyphEntry {
                    page: 1,
                    color: false,
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                    bearing_x: 0,
                    bearing_y: 0
                },
                cell: 0,
                x_offset: 0.0,
            };
            count
        ],
    };
    (key, run)
}

// Independently enumerate stored capacities rather than trusting charges.
fn recount_run_cache(cache: &ShapedRunCache) -> usize {
    cache
        .entries
        .iter()
        .map(|(key, (run, _))| {
            std::mem::size_of::<LigatureKey>()
                + std::mem::size_of::<(LigatureRun, usize)>()
                + key.text.capacity()
                + run.glyphs.capacity() * std::mem::size_of::<ShapedGlyph>()
        })
        .sum()
}

#[test]
fn shaped_cache_accounts_capacities_replacement_retention_and_clear() {
    let mut cache = ShapedRunCache::new(64 * 1024);
    let (mut key, run) = cache_fixture("same", 3);
    key.text.reserve(400);
    cache.insert(key.clone(), &run);
    assert_eq!(cache.bytes, recount_run_cache(&cache));
    let before = cache.bytes;
    cache.insert(key, &run);
    assert_eq!(cache.len(), 1);
    assert!(
        cache.bytes > before,
        "replacement must account the moved key's reserved capacity"
    );
    assert_eq!(cache.bytes, recount_run_cache(&cache));
    let (other_key, mut other_run) = cache_fixture("other", 1);
    other_run.glyphs[0].entry.page = 2;
    cache.insert(other_key, &other_run);
    cache.retain(|run| run.glyphs.iter().all(|glyph| glyph.entry.page != 1));
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.bytes, recount_run_cache(&cache));
    cache.clear();
    assert_eq!(cache.bytes, 0);
    assert_eq!(cache.len(), 0);
}

#[test]
fn shaped_cache_byte_pressure_and_oversized_bypass_preserve_limits() {
    let (key, run) = cache_fixture("first", 100);
    let mut measuring = ShapedRunCache::new(usize::MAX);
    measuring.insert(key.clone(), &run);
    let size = recount_run_cache(&measuring);
    for budget in [size, size * 2] {
        let mut cache = ShapedRunCache::new(budget);
        cache.insert(key.clone(), &run);
        assert!(cache.get(&key).is_some());
        let mut evictions = 0;
        for name in ["other", "third", "forth"] {
            let (key, run) = cache_fixture(name, 100);
            evictions += cache.insert(key, &run);
            assert!(cache.bytes <= budget);
            assert_eq!(cache.bytes, recount_run_cache(&cache));
        }
        assert!(evictions > 0 && cache.len() < MAX_SHAPED_RUNS);
        let before = (cache.len(), cache.bytes);
        let (huge_key, huge_run) = cache_fixture(&"x".repeat(budget + 1), 100);
        assert_eq!(cache.insert(huge_key, &huge_run), 0);
        assert_eq!((cache.len(), cache.bytes), before);
        assert_eq!(cache.skipped, 1);
        assert_eq!(
            huge_run.glyphs.len(),
            100,
            "bypass must not truncate the caller's output"
        );
    }
}

#[test]
fn shaped_cache_count_ceiling_remains_independent_of_byte_ceiling() {
    let mut cache = ShapedRunCache::new(MAX_SHAPED_RUN_BYTES);
    for index in 0..MAX_SHAPED_RUNS {
        let (key, run) = cache_fixture(&format!("{index:04}"), 1);
        assert_eq!(cache.insert(key, &run), 0);
    }
    assert_eq!(cache.len(), MAX_SHAPED_RUNS);
    assert!(cache.bytes < MAX_SHAPED_RUN_BYTES);
    let (key, run) = cache_fixture("extra", 1);
    assert_eq!(cache.insert(key, &run), MAX_SHAPED_RUNS);
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.bytes, recount_run_cache(&cache));
}

fn shape_signature(font_data: Vec<u8>, text: &str, ligatures: bool) -> Vec<(u16, usize, usize)> {
    shape_signature_with_overrides(font_data, text, ligatures, &BTreeMap::new())
}

fn shape_signature_with_overrides(
    font_data: Vec<u8>,
    text: &str,
    ligatures: bool,
    overrides: &BTreeMap<String, u32>,
) -> Vec<(u16, usize, usize)> {
    let source = cosmic_text::fontdb::Source::Binary(Arc::new(font_data));
    let mut font_system = FontSystem::new_with_fonts([source]);
    let mut buffer = Buffer::new(&mut font_system, Metrics::new(16.0, 20.0));
    let overrides = compile_font_feature_overrides(overrides);
    let attrs = Attrs::new()
        .family(Family::Name("Fira Mono"))
        .font_features(terminal_font_features(ligatures, &overrides));
    buffer.set_text(text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(&mut font_system, false);
    buffer
        .layout_runs()
        .flat_map(|run| run.glyphs.iter())
        .map(|glyph| (glyph.glyph_id, glyph.start, glyph.end))
        .collect()
}

fn variable_weight_raster(weight: Weight) -> (CacheKey, Vec<u8>, bool) {
    let source = cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
        "InterVariable.ttf",
    )));
    let mut database = cosmic_text::fontdb::Database::new();
    database.load_font_source(source);
    let mut font_system = FontSystem::new_with_locale_and_db("en-US".to_string(), database);
    let mut buffer = Buffer::new(&mut font_system, Metrics::new(32.0, 38.4));
    buffer.set_text(
        "M",
        &Attrs::new()
            .family(Family::Name("Inter Variable"))
            .weight(weight),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(&mut font_system, false);
    let cache_key = buffer
        .layout_runs()
        .flat_map(|run| run.glyphs.iter())
        .find(|glyph| glyph.glyph_id != 0)
        .expect("Inter Variable should shape M")
        .physical((0.0, 0.0), 1.0)
        .cache_key;
    let has_weight_axis =
        font_has_weight_axis(&mut font_system, cache_key.font_id, cache_key.font_weight);
    let image = SwashCache::new()
        .get_image(&mut font_system, cache_key)
        .as_ref()
        .expect("Inter Variable should rasterize M")
        .clone();
    (cache_key, image.data, has_weight_axis)
}

#[test]
fn configured_font_resolution_keeps_generic_primary() {
    let font_system = FontSystem::new();
    assert_eq!(
        resolve_configured_family(&font_system, "monospace", &[]),
        "monospace"
    );
}

#[test]
fn configured_font_resolution_uses_ordered_generic_fallback() {
    let font_system = FontSystem::new();
    let fallbacks = vec!["also-not-installed".into(), "monospace".into()];
    assert_eq!(
        resolve_configured_family(&font_system, "definitely-not-installed", &fallbacks),
        "monospace"
    );
}

#[test]
fn family_span_coalescing_resolves_only_adjacent_identical_graphemes() {
    for text in [
        String::new(),
        "W".repeat(10_000),
        "abcabc".into(),
        "e\u{301}e\u{301}éé👩\u{200d}💻👩\u{200d}💻a".into(),
        "\u{FE0F}\u{FE0F}شَشَ\r\n\r\n".into(),
    ] {
        let mut resolutions = Vec::new();
        let spans = coalesced_family_spans(&text, |grapheme| {
            resolutions.push(grapheme.to_owned());
            "one family"
        });
        let mut expected = text.graphemes(true).collect::<Vec<_>>();
        expected.dedup();
        assert_eq!(resolutions, expected);
        if text.is_empty() {
            assert!(spans.is_empty());
        } else {
            assert_eq!(spans, vec![(0..text.len(), "one family".into())]);
        }
    }

    let spans = coalesced_family_spans("aaaشَشَbbشَ", |grapheme| {
        if grapheme.starts_with('ش') {
            "Arabic"
        } else {
            "Latin"
        }
    });
    assert_eq!(
        spans,
        vec![
            (0..3, "Latin".into()),
            (3..11, "Arabic".into()),
            (11..13, "Latin".into()),
            (13..17, "Arabic".into()),
        ]
    );
    // Resolutions are local, so a subsequent font policy takes effect.
    assert_eq!(
        coalesced_family_spans("aaa", |_| "changed"),
        vec![(0..3, "changed".into())]
    );
}

#[test]
fn coalesced_font_routing_matches_uncached_reference_for_complete_graphemes() {
    let mut database = cosmic_text::fontdb::Database::new();
    for name in ["FiraMono-Medium.ttf", "NotoSansArabic.ttf"] {
        database.load_font_data(deterministic_test_font_data_named(name));
    }
    let mut fonts = FontSystem::new_with_locale_and_db("en-US".into(), database);
    let families = collect_configured_families(&fonts, "Fira Mono", &["Noto Sans Arabic".into()]);
    let rules = collect_configured_range_overrides(
        &fonts,
        &[
            FontRangeOverride {
                start: 0x30,
                end: 0x39,
                family: "Noto Sans Arabic".into(),
            },
            // Incomplete override must fall through to the global Arabic face.
            FontRangeOverride {
                start: 0x600,
                end: 0x6ff,
                family: "Fira Mono".into(),
            },
            FontRangeOverride {
                start: 0xe000,
                end: 0xf8ff,
                family: "Missing".into(),
            },
        ],
    );
    for configured in [families.as_slice(), &[]] {
        for overrides in [rules.as_slice(), &[]] {
            for text in [
                "",
                "WWWW001122",
                "abcشَشَabc",
                "e\u{301}e\u{301}éé",
                "👩\u{200d}💻👩\u{200d}💻",
                "\u{FE0F}\u{FE0F}ش",
                "\u{E000}\u{E000}x",
                "\r\n\r\n",
            ] {
                // Former per-grapheme owned-name algorithm, intentionally
                // independent of the new coalescer/memoization helper.
                let mut expected: Vec<(Range<usize>, String)> = Vec::new();
                for (start, grapheme) in text.grapheme_indices(true) {
                    let family =
                        range_override_family_with_coverage(&mut fonts, overrides, grapheme)
                            .map(str::to_owned)
                            .or_else(|| {
                                first_configured_family_with_coverage(
                                    &mut fonts, configured, grapheme,
                                )
                                .map(|index| configured[index].name.clone())
                            })
                            .unwrap_or_else(|| "Fira Mono".to_owned());
                    let end = start + grapheme.len();
                    if let Some((range, previous)) = expected.last_mut()
                        && previous == &family
                    {
                        range.end = end;
                    } else {
                        expected.push((start..end, family));
                    }
                }
                assert_eq!(
                    configured_family_spans(&mut fonts, configured, overrides, text, "Fira Mono"),
                    expected
                );
            }
        }
    }
}

#[test]
fn configured_fallback_order_selects_complete_grapheme_coverage() {
    let sources = [
        cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
            "FiraMono-Medium.ttf",
        ))),
        cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
            "NotoSansArabic.ttf",
        ))),
    ];
    let mut font_system = FontSystem::new_with_fonts(sources);
    let fallbacks = vec![
        "Definitely Missing".to_string(),
        "Noto Sans Arabic".to_string(),
    ];
    let families = collect_configured_families(&font_system, "Fira Mono", &fallbacks);

    assert_eq!(
        first_configured_family_with_coverage(&mut font_system, &families, "abc"),
        Some(0)
    );
    let arabic = first_configured_family_with_coverage(&mut font_system, &families, "ش")
        .expect("the configured Arabic fallback should cover the grapheme");
    assert_eq!(families[arabic].name, "Noto Sans Arabic");

    assert_eq!(
        configured_family_spans(&mut font_system, &families, &[], "abcش", "Fira Mono"),
        vec![
            (0..3, "Fira Mono".to_string()),
            (3..5, "Noto Sans Arabic".to_string())
        ]
    );
}

#[test]
fn range_overrides_precede_global_fallback_without_splitting_graphemes() {
    let sources = [
        cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
            "FiraMono-Medium.ttf",
        ))),
        cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
            "NotoSansArabic.ttf",
        ))),
    ];
    let mut font_system = FontSystem::new_with_fonts(sources);
    let fallbacks = vec!["Noto Sans Arabic".to_string()];
    let families = collect_configured_families(&font_system, "Fira Mono", &fallbacks);
    let rules = collect_configured_range_overrides(
        &font_system,
        &[
            FontRangeOverride {
                start: 0x0600,
                end: 0x06FF,
                family: "Noto Sans Arabic".into(),
            },
            // Deliberately supplied out of interval order; validated app
            // config guarantees no overlap, while compilation sorts once.
            FontRangeOverride {
                start: 0x0030,
                end: 0x0039,
                family: "Noto Sans Arabic".into(),
            },
        ],
    );

    assert_eq!(
        range_override_family_with_coverage(&mut font_system, &rules, "0"),
        Some("Noto Sans Arabic")
    );
    assert_eq!(
        range_override_family_with_coverage(&mut font_system, &rules, "\u{FE0F}ش"),
        Some("Noto Sans Arabic"),
        "default-ignorable selectors must not own range lookup"
    );
    assert_eq!(
        configured_family_spans(&mut font_system, &families, &rules, "01ش", "Fira Mono"),
        vec![(0..4, "Noto Sans Arabic".to_string())]
    );
    assert_eq!(
        range_override_family_with_coverage(&mut font_system, &rules, "0🧡"),
        None,
        "a mapped family that lacks part of the grapheme must fall through"
    );

    let incomplete = collect_configured_range_overrides(
        &font_system,
        &[FontRangeOverride {
            start: 0x0600,
            end: 0x06FF,
            family: "Fira Mono".into(),
        }],
    );
    assert_eq!(
        configured_family_spans(&mut font_system, &families, &incomplete, "ش", "Fira Mono"),
        vec![(0..2, "Noto Sans Arabic".to_string())],
        "incomplete mapped faces must retain the proven global fallback chain"
    );
}

#[test]
fn missing_range_family_and_unmatched_boundaries_fall_through() {
    let mut font_system = FontSystem::new_with_fonts([cosmic_text::fontdb::Source::Binary(
        Arc::new(deterministic_test_font_data_named("FiraMono-Medium.ttf")),
    )]);
    let rules = collect_configured_range_overrides(
        &font_system,
        &[FontRangeOverride {
            start: 0xE000,
            end: 0xF8FF,
            family: "Definitely Missing".into(),
        }],
    );
    assert_eq!(
        range_override_family_with_coverage(&mut font_system, &rules, "\u{E000}"),
        None
    );
    assert_eq!(
        range_override_family_with_coverage(&mut font_system, &rules, "\u{D7FF}"),
        None
    );
    assert_eq!(
        range_override_family_with_coverage(&mut font_system, &rules, "\u{F900}"),
        None
    );
}

#[test]
fn coverage_report_matches_range_and_ordered_fallback_precedence() {
    let sources = [
        cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
            "FiraMono-Medium.ttf",
        ))),
        cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
            "NotoSansArabic.ttf",
        ))),
    ];
    let mut font_system = FontSystem::new_with_fonts(sources);
    let rules = collect_configured_range_overrides(
        &font_system,
        &[
            FontRangeOverride {
                start: 0x0600,
                end: 0x06FF,
                family: "Fira Mono".into(),
            },
            FontRangeOverride {
                start: 0x0030,
                end: 0x0039,
                family: "Noto Sans Arabic".into(),
            },
        ],
    );
    let fallbacks = vec!["Fira Mono".into(), "Noto Sans Arabic".into()];
    let report = diagnose_font_coverage(
        &mut font_system,
        Metrics::new(14.0, 16.8),
        Weight::NORMAL,
        "Missing Primary",
        &fallbacks,
        &rules,
        "Fira Mono",
        "0شّ",
    )
    .unwrap();
    assert_eq!(report.entries.len(), 2);
    assert_eq!(
        report.entries[0].route,
        FontCoverageRoute::Explicit {
            source: FontCoverageSource::Range {
                start: 0x0030,
                end: 0x0039,
            },
            family: "Noto Sans Arabic".into(),
        }
    );
    assert_eq!(report.entries[0].attempts.len(), 1);
    assert!(report.entries[0].attempts[0].installed);
    assert!(report.entries[0].attempts[0].complete);

    let arabic = &report.entries[1];
    assert_eq!(arabic.grapheme, "شّ");
    assert_eq!(arabic.attempts.len(), 4);
    assert_eq!(
        arabic.attempts[0].source,
        FontCoverageSource::Range {
            start: 0x0600,
            end: 0x06FF,
        }
    );
    assert!(arabic.attempts[0].installed);
    assert!(!arabic.attempts[0].complete);
    assert_eq!(arabic.attempts[1].source, FontCoverageSource::Primary);
    assert!(!arabic.attempts[1].installed);
    assert_eq!(
        arabic.route,
        FontCoverageRoute::Explicit {
            source: FontCoverageSource::Fallback { index: 2 },
            family: "Noto Sans Arabic".into(),
        }
    );
}

#[test]
fn a_character_no_configured_font_has_falls_back_by_script() {
    let sources = [
        cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
            "FiraMono-Medium.ttf",
        ))),
        cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
            "NotoSansArabic.ttf",
        ))),
    ];
    let mut font_system = FontSystem::new_with_fonts(sources);
    let metrics = Metrics::new(14.0, 16.8);
    let attrs = Attrs::new().family(Family::Name("Fira Mono"));
    let arabic = |font_system: &mut FontSystem, shaping| {
        char_glyph(font_system, metrics, 'ش', &attrs, shaping)
    };
    // Basic shaping looks only at the named family, as the wide-cell
    // path once did for its last resort, which drew kana as boxes.
    assert!(arabic(&mut font_system, Shaping::Basic).is_none());
    let (_, font) = arabic(&mut font_system, Shaping::Advanced).expect("a fallback font");
    let face = font_system.db().face(font).unwrap();
    assert!(
        face.families
            .iter()
            .any(|(name, _)| name == "Noto Sans Arabic")
    );
}

#[test]
fn coverage_report_names_platform_fallback_and_missing_results() {
    let sources = [
        cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
            "FiraMono-Medium.ttf",
        ))),
        cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data_named(
            "NotoSansArabic.ttf",
        ))),
    ];
    let mut font_system = FontSystem::new_with_fonts(sources);
    let metrics = Metrics::new(14.0, 16.8);
    let platform = diagnose_font_coverage(
        &mut font_system,
        metrics,
        Weight::NORMAL,
        "Fira Mono",
        &[],
        &[],
        "Fira Mono",
        "ش",
    )
    .unwrap();
    assert!(matches!(
        &platform.entries[0].route,
        FontCoverageRoute::Platform { families, .. }
            if families == &["Noto Sans Arabic".to_string()]
    ));

    let missing = diagnose_font_coverage(
        &mut font_system,
        metrics,
        Weight::NORMAL,
        "Fira Mono",
        &[],
        &[],
        "Fira Mono",
        "\u{10FFFF}",
    )
    .unwrap();
    assert_eq!(missing.entries[0].route, FontCoverageRoute::Missing);

    let ignorable = diagnose_font_coverage(
        &mut font_system,
        metrics,
        Weight::NORMAL,
        "Fira Mono",
        &[],
        &[],
        "Fira Mono",
        "\u{FE0F}",
    )
    .unwrap();
    assert_eq!(
        ignorable.entries[0].route,
        FontCoverageRoute::DefaultIgnorable {
            family: "Fira Mono".into()
        }
    );
}

#[test]
fn font_default_ignorable_predicate_matches_shaping_semantics() {
    for character in [
        '\u{00AD}',
        '\u{034F}',
        '\u{061C}',
        '\u{17B4}',
        '\u{180E}',
        '\u{200B}',
        '\u{206F}',
        '\u{FE0F}',
        '\u{FEFF}',
        '\u{FFF8}',
        '\u{1D17A}',
        '\u{E0000}',
        '\u{E0FFF}',
    ] {
        assert!(
            is_font_default_ignorable(character),
            "U+{:04X} should be default-ignorable",
            u32::from(character)
        );
    }
    for character in [
        'A',
        '\u{00AE}',
        // HarfBuzz preserves these default-ignorable codepoints because
        // fonts use them as spacing glyphs or shaping controls.
        '\u{115F}',
        '\u{1161}',
        '\u{180F}',
        '\u{1810}',
        '\u{2010}',
        '\u{2070}',
        '\u{3164}',
        '\u{FFA0}',
        '\u{1BCA0}',
        '\u{E1000}',
    ] {
        assert!(
            !is_font_default_ignorable(character),
            "U+{:04X} should remain substantive",
            u32::from(character)
        );
    }
}

#[test]
fn coverage_report_bounds_input_without_splitting_the_retained_graphemes() {
    let mut font_system = FontSystem::new_with_fonts([cosmic_text::fontdb::Source::Binary(
        Arc::new(deterministic_test_font_data_named("FiraMono-Medium.ttf")),
    )]);
    let text = "a".repeat(MAX_FONT_COVERAGE_GRAPHEMES + 1);
    let report = diagnose_font_coverage(
        &mut font_system,
        Metrics::new(14.0, 16.8),
        Weight::NORMAL,
        "Fira Mono",
        &[],
        &[],
        "Fira Mono",
        &text,
    )
    .unwrap();
    assert_eq!(report.entries.len(), MAX_FONT_COVERAGE_GRAPHEMES);
    assert!(report.graphemes_truncated);
    assert!(report.entries.iter().all(|entry| entry.grapheme == "a"));
    assert!(
        diagnose_font_coverage(
            &mut font_system,
            Metrics::new(14.0, 16.8),
            Weight::NORMAL,
            "Fira Mono",
            &[],
            &[],
            "Fira Mono",
            &"a".repeat(MAX_FONT_COVERAGE_INPUT_BYTES + 1),
        )
        .is_err()
    );
    assert!(
        diagnose_font_coverage(
            &mut font_system,
            Metrics::new(14.0, 16.8),
            Weight::NORMAL,
            "Fira Mono",
            &[],
            &[],
            "Fira Mono",
            "bad\ninput",
        )
        .is_err()
    );
}

#[test]
fn cell_height_uses_configured_line_height() {
    // A bundled face, so the test doesn't depend on the machine's fonts.
    let mut font_system = FontSystem::new_with_fonts([cosmic_text::fontdb::Source::Binary(
        Arc::new(deterministic_test_font_data_named("FiraMono-Medium.ttf")),
    )]);
    let metrics = Metrics::new(14.0, 21.0);
    let cell = measure_cell(&mut font_system, &metrics, "Fira Mono", Weight::NORMAL);
    assert_eq!(cell.height, 21.0);
    // The baseline comes from the face, not a fixed fraction of the cell,
    // and is whole so glyph rows cannot straddle pixel boundaries.
    assert_eq!(cell.baseline.fract(), 0.0);
    assert!(cell.baseline > 0.0 && cell.baseline <= cell.height);
}

#[test]
fn rgba_glyphs_preserve_color_while_masks_remain_compact() {
    let rgba = [10, 20, 30, 40, 50, 60, 70, 80];
    assert_eq!(
        glyph_pixels(SwashContent::Color, &rgba, 2),
        Some(RasterPixels::Color(rgba.to_vec()))
    );
    assert_eq!(
        glyph_pixels(SwashContent::SubpixelMask, &rgba, 2),
        Some(RasterPixels::Mask(vec![40, 80]))
    );
    assert_eq!(
        glyph_pixels(SwashContent::Mask, &[12, 34], 2),
        Some(RasterPixels::Mask(vec![12, 34]))
    );
    assert_eq!(glyph_pixels(SwashContent::Color, &[1, 2, 3], 1), None);
}

#[test]
fn sgr_face_requests_are_part_of_every_glyph_cache_key() {
    let regular = FontStyle::default();
    let bold_italic = FontStyle {
        bold: true,
        italic: true,
    };
    assert_ne!(
        GlyphKey::new('x', 14.0, regular, 400),
        GlyphKey::new('x', 14.0, bold_italic, 700)
    );
    assert_ne!(
        LigatureKey::new(
            "e\u{301}",
            14.0,
            regular,
            GlyphPlacement::CellGrid,
            TextFace::Terminal
        ),
        LigatureKey::new(
            "e\u{301}",
            14.0,
            bold_italic,
            GlyphPlacement::CellGrid,
            TextFace::Terminal
        )
    );
    let grid = |placement| LigatureKey::new("ab", 14.0, regular, placement, TextFace::Terminal);
    assert_ne!(
        grid(GlyphPlacement::CellGrid),
        grid(GlyphPlacement::Shaped),
        "cell-grid and shaped offsets must not share a cached run"
    );
    let attrs = font_attrs(Family::Monospace, bold_italic, Weight(350), Weight(775));
    assert_eq!(attrs.weight, Weight(775));
    assert_eq!(attrs.style, Style::Italic);

    assert_ne!(
        GlyphKey::new('x', 14.0, regular, 350),
        GlyphKey::new('x', 14.0, regular, 450),
        "normal weight must participate in scalar cache identity"
    );
    let shaped = |size, face| LigatureKey::new("fi", size, regular, GlyphPlacement::Shaped, face);
    assert_ne!(
        shaped(14.0, TextFace::Terminal),
        shaped(28.0, TextFace::Terminal),
        "OSC 66 scaled runs must not share a cached run"
    );
    assert_ne!(
        shaped(14.0, TextFace::Terminal),
        shaped(14.0, TextFace::UiSans),
        "terminal and chrome text must not share a cached run"
    );
}

#[test]
fn variable_font_weight_reaches_shaping_and_rasterization_without_double_bold() {
    let (light_key, light_pixels, light_has_axis) = variable_weight_raster(Weight(300));
    let (heavy_key, heavy_pixels, heavy_has_axis) = variable_weight_raster(Weight(800));

    assert_eq!(light_key.font_weight, Weight(300));
    assert_eq!(heavy_key.font_weight, Weight(800));
    assert_ne!(light_key, heavy_key);
    assert_ne!(light_pixels, heavy_pixels);
    assert!(light_has_axis && heavy_has_axis);
    assert!(!should_synthesize_bold(
        FontStyle {
            bold: true,
            italic: false,
        },
        Weight(800),
        true,
        true,
    ));
    assert!(should_synthesize_bold(
        FontStyle {
            bold: true,
            italic: false,
        },
        Weight(800),
        true,
        false,
    ));
}

#[test]
fn synthetic_bold_is_a_bounded_one_pixel_embolden() {
    let (width, mask) = embolden_alpha_mask(2, 2, &[0, 200, 50, 0]);
    assert_eq!(width, 3);
    assert_eq!(mask, vec![0, 200, 200, 50, 50, 0]);
}

#[test]
fn tiny_paged_atlas_grows_then_evicts_the_lru_unpinned_page() {
    let mut atlas = PagedAtlasAllocator::new(16, 3);
    // Page zero is permanently protected by the visible fallback glyph.
    let fallback = atlas.allocate(14, 14).expect("fallback page");
    assert_eq!(fallback.page, 0);
    let second = atlas.allocate(14, 14).expect("second page");
    let third = atlas.allocate(14, 14).expect("third page");
    assert_eq!((second.page, third.page), (1, 2));
    assert_eq!(atlas.metrics(0, 0, 1).pages, 3);

    atlas.begin_frame();
    atlas.pin(2);
    let recycled = atlas.allocate(14, 14).expect("recycled page");
    assert_eq!(recycled.page, 1);
    assert_eq!(recycled.evicted_page, Some(1));
    assert_eq!(atlas.metrics(0, 0, 1).evictions, 1);
}

#[test]
fn tiny_paged_atlas_never_recycles_a_page_used_in_the_current_frame() {
    let mut atlas = PagedAtlasAllocator::new(16, 2);
    atlas.allocate(14, 14).unwrap();
    atlas.allocate(14, 14).unwrap();
    atlas.begin_frame();
    atlas.pin(1);
    let generation = atlas.generation;
    assert_eq!(
        atlas.allocate(14, 14),
        Err(AtlasAllocationError::FramePinned)
    );
    assert_eq!(atlas.metrics(0, 0, 1).allocation_failures, 1);

    // The failed frame may have cached a row containing the temporary
    // replacement glyph. Invalidate that row only after this frame's pins
    // are released, then allow the real glyph to reclaim the old page.
    assert_eq!(atlas.generation, generation);
    atlas.begin_frame();
    assert_ne!(atlas.generation, generation);
    assert!(atlas.allocate(14, 14).is_ok());
}

#[test]
fn oversized_and_frame_pinned_allocations_are_distinguished() {
    let mut atlas = PagedAtlasAllocator::new(16, 2);
    assert_eq!(atlas.allocate(16, 1), Err(AtlasAllocationError::TooLarge));

    atlas.allocate(14, 14).unwrap();
    atlas.allocate(14, 14).unwrap();
    atlas.begin_frame();
    atlas.pin(1);
    assert_eq!(
        atlas.allocate(14, 14),
        Err(AtlasAllocationError::FramePinned)
    );
}

#[test]
fn color_page_zero_is_recyclable_and_has_a_disjoint_row_cache_key() {
    let mut atlas = PagedAtlasAllocator::recyclable(16, 1);
    let first = atlas.allocate(14, 14).unwrap();
    assert_eq!(first.page, 0);
    atlas.begin_frame();
    let recycled = atlas.allocate(14, 14).unwrap();
    assert_eq!(recycled.evicted_page, Some(0));

    let mask = GlyphEntry {
        page: 0,
        color: false,
        x: 0,
        y: 0,
        width: 1,
        height: 1,
        bearing_x: 0,
        bearing_y: 0,
    };
    assert_ne!(
        mask.page_key(),
        GlyphEntry {
            color: true,
            ..mask
        }
        .page_key()
    );
}

#[test]
fn atlas_capacity_is_bounded_and_reset_advances_generation() {
    let mut atlas = PagedAtlasAllocator::new(64, 4);
    assert_eq!(atlas.metrics(3, 2, 1).capacity_bytes, 16_384);
    assert_eq!(atlas.metrics(3, 2, 1).hits, 3);
    let generation = atlas.generation;
    atlas.reset();
    assert_ne!(atlas.generation, generation);
    assert_eq!(atlas.metrics(0, 0, 1).pages, 1);
}

#[test]
fn explicit_feature_toggle_changes_a_known_ligature_font_run() {
    let data = deterministic_test_font_data();
    let changed = ["fi", "ffi"]
        .into_iter()
        .filter(|text| {
            shape_signature(data.clone(), text, true) != shape_signature(data.clone(), text, false)
        })
        .collect::<Vec<_>>();
    assert!(
        !changed.is_empty(),
        "test font did not expose any OpenType ligature difference"
    );
}

#[test]
fn explicit_font_features_follow_the_ligature_preset_and_enter_cache_identity() {
    let disabled = ["liga", "clig", "calt", "dlig"]
        .into_iter()
        .map(|tag| (tag.to_string(), 0))
        .collect::<BTreeMap<_, _>>();
    let enabled = ["liga", "clig", "calt", "dlig"]
        .into_iter()
        .map(|tag| (tag.to_string(), 1))
        .collect::<BTreeMap<_, _>>();
    let data = deterministic_test_font_data();
    let text = ["fi", "ffi"]
        .into_iter()
        .find(|text| {
            shape_signature(data.clone(), text, true) != shape_signature(data.clone(), text, false)
        })
        .expect("test font should expose a ligature difference");
    assert_eq!(
        shape_signature_with_overrides(data.clone(), text, true, &disabled),
        shape_signature(data.clone(), text, false),
        "explicit zero values must override the enabled preset"
    );
    assert_eq!(
        shape_signature_with_overrides(data.clone(), text, false, &enabled),
        shape_signature(data, text, true),
        "explicit one values must override the disabled preset"
    );
}

#[test]
fn cell_grid_placement_gives_wide_characters_two_cells() {
    // Whatever font draws the kana, it starts one cell after `a` and
    // takes two, so `b` starts in the fourth cell.
    let glyphs = fira_mono_layout("aかb");
    let cells: Vec<u32> = place_glyphs("aかb", &glyphs, GlyphPlacement::CellGrid)
        .iter()
        .map(|glyph| glyph.cell)
        .collect();
    assert_eq!(cells, [0, 1, 3]);
}

fn fira_mono_layout(text: &str) -> Vec<cosmic_text::LayoutGlyph> {
    let source = cosmic_text::fontdb::Source::Binary(Arc::new(deterministic_test_font_data()));
    let mut font_system = FontSystem::new_with_fonts([source]);
    let mut buffer = Buffer::new(&mut font_system, Metrics::new(16.0, 20.0));
    buffer.set_text(
        text,
        &Attrs::new().family(Family::Name("Fira Mono")),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(&mut font_system, false);
    buffer
        .layout_runs()
        .flat_map(|run| run.glyphs.iter().cloned())
        .collect()
}

#[test]
fn cell_grid_placement_puts_every_glyph_on_its_whole_pixel_cell_origin() {
    let text = "drwxr-xr-x 17";
    let glyphs = fira_mono_layout(text);
    assert!(
        glyphs.iter().any(|glyph| glyph.x.fract() != 0.0),
        "the fixture must exercise fractional shaped advances"
    );
    let placed = place_glyphs(text, &glyphs, GlyphPlacement::CellGrid);
    assert_eq!(placed.len(), text.chars().count());
    for (index, glyph) in placed.iter().enumerate() {
        assert_eq!(glyph.cell, index as u32);
        assert_eq!(glyph.x_offset, 0.0);
        assert_eq!(glyph.cache_key.x_bin, cosmic_text::SubpixelBin::Zero);
    }
}

#[test]
fn shaped_placement_applies_a_fractional_position_exactly_once() {
    let text = "drwxr-xr-x";
    let glyphs = fira_mono_layout(text);
    let placed = place_glyphs(text, &glyphs, GlyphPlacement::Shaped);
    assert_eq!(placed.len(), glyphs.len());
    for (glyph, placed) in glyphs.iter().zip(placed) {
        assert_eq!(placed.cell, 0);
        assert_eq!(placed.x_offset.fract(), 0.0);
        // The quad offset plus the rasterized bin reproduce the shaped
        // position to the quarter-pixel bin resolution.
        let expected = glyph.x + glyph.font_size * glyph.x_offset;
        let presented = placed.x_offset + placed.cache_key.x_bin.as_float();
        assert!(
            (presented - expected).abs() <= 0.125,
            "{presented} presented for shaped position {expected}"
        );
    }
}
