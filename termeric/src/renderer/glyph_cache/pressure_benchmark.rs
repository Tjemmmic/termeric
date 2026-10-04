//! Paired real-shaping workload: byte-pressure churn versus retained warm runs.
//! Only compiled in tests; the larger reference budget is never an app option.

use super::*;
use sha2::{Digest, Sha256};
use std::time::Instant;

const RUNS: usize = 128;
const WIDTH: usize = 2_048;
const REFERENCE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct PressureBenchmark {
    pub font_sha256: String,
    pub pressure_ms: Vec<f64>,
    pub reference_ms: Vec<f64>,
    pub pressure_bytes: Vec<f64>,
    pub reference_bytes: Vec<f64>,
    pub pressure_evictions: Vec<f64>,
    pub pressure_rasterizations: Vec<f64>,
}

fn fixture() -> Vec<String> {
    (0..RUNS)
        .map(|index| format!("{index:04}{}", "W".repeat(WIDTH - 4)))
        .collect()
}

fn shape_pass(cache: &mut GlyphCache, queue: &Queue, inputs: &[String]) -> Vec<LigatureRun> {
    inputs
        .iter()
        .map(|text| {
            cache
                .shape_run_styled(text, FontStyle::default(), queue)
                .expect("pressure fixture must shape")
        })
        .collect()
}

fn assert_same_runs(expected: &[LigatureRun], actual: &[LigatureRun]) {
    assert_eq!(expected.len(), RUNS);
    assert_eq!(actual.len(), RUNS);
    for (expected, actual) in expected.iter().zip(actual) {
        assert_eq!(expected.glyphs.len(), WIDTH);
        assert_eq!(actual.glyphs.len(), WIDTH);
        for (expected, actual) in expected.glyphs.iter().zip(&actual.glyphs) {
            assert_eq!(expected.entry, actual.entry);
            assert_eq!(expected.x_offset.to_bits(), actual.x_offset.to_bits());
        }
    }
}

pub(crate) fn measure(device: &Device, queue: &Queue) -> Result<PressureBenchmark> {
    let mut cache = GlyphCache::for_test(device, queue, 16.0, true)?;
    let font = deterministic_test_font_data();
    let font_sha256 = format!("{:x}", Sha256::digest(&font));
    // A same-named installed face must not replace the fingerprinted fixture.
    cache.font_system =
        FontSystem::new_with_locale_and_db("en-US".into(), cosmic_text::fontdb::Database::new());
    cache.install_test_font(font, "Fira Mono", queue);
    cache.begin_frame();
    let inputs = fixture();
    cache.set_shaping_byte_limit_for_test(REFERENCE_BYTES);
    let expected = shape_pass(&mut cache, queue, &inputs);
    // Warm all physical glyphs and wait for uploads before CPU-only timings.
    queue.submit([]);
    device.poll(Maintain::Wait);
    let atlas_before = cache.atlas_metrics();
    let raster_before = cache.rasterizations();
    let mut result = PressureBenchmark {
        font_sha256,
        ..PressureBenchmark::default()
    };

    // One unrecorded pair, then five recorded pairs with alternating order.
    for sample in 0..6 {
        let budgets = if sample % 2 == 0 {
            [MAX_SHAPED_RUN_BYTES, REFERENCE_BYTES]
        } else {
            [REFERENCE_BYTES, MAX_SHAPED_RUN_BYTES]
        };
        for budget in budgets {
            cache.set_shaping_byte_limit_for_test(budget);
            let warmup = shape_pass(&mut cache, queue, &inputs);
            assert_same_runs(&expected, &warmup);
            drop(warmup);
            let before = cache.shaping_cache_metrics();
            let raster = cache.rasterizations();
            let started = Instant::now();
            let actual = shape_pass(&mut cache, queue, &inputs);
            let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
            let after = cache.shaping_cache_metrics();
            // Diagnostics, geometry comparisons, and output destruction are
            // outside timing, identically for each side of the pair.
            assert_same_runs(&expected, &actual);
            assert!(after.retained_bytes <= budget);
            assert_eq!(after.deferred_entries, 0);
            assert_eq!(after.skipped_runs, before.skipped_runs);
            let rasterizations = cache.rasterizations() - raster;
            assert_eq!(
                rasterizations, 0,
                "shaping churn must not rerasterize atlas hits"
            );
            let evictions = after.evictions - before.evictions;
            if budget == MAX_SHAPED_RUN_BYTES {
                assert!(
                    evictions > 0,
                    "fixture must cross the production byte budget"
                );
                assert_eq!(after.misses - before.misses, RUNS as u64);
                if sample > 0 {
                    result.pressure_ms.push(elapsed_ms);
                    result.pressure_bytes.push(after.retained_bytes as f64);
                    result.pressure_evictions.push(evictions as f64);
                    result.pressure_rasterizations.push(rasterizations as f64);
                }
            } else {
                assert_eq!(after.entries, RUNS);
                assert!(after.retained_bytes > MAX_SHAPED_RUN_BYTES);
                assert_eq!(evictions, 0);
                assert_eq!(after.hits - before.hits, RUNS as u64);
                assert_eq!(after.misses, before.misses);
                if sample > 0 {
                    result.reference_ms.push(elapsed_ms);
                    result.reference_bytes.push(after.retained_bytes as f64);
                }
            }
        }
    }
    assert_eq!(cache.atlas_metrics().evictions, atlas_before.evictions);
    assert_eq!(cache.rasterizations(), raster_before);
    Ok(result)
}

#[test]
fn pressure_fixture_exceeds_byte_not_entry_ceiling() {
    let inputs = fixture();
    assert_eq!(inputs.len(), RUNS);
    assert!(inputs.len() < MAX_SHAPED_RUNS);
    assert!(
        inputs
            .iter()
            .all(|text| text.len() == WIDTH && text.is_ascii())
    );
    assert_eq!(inputs.iter().collect::<HashSet<_>>().len(), RUNS);
    let minimum_owned_bytes = RUNS * WIDTH * (std::mem::size_of::<ShapedGlyph>() + 1);
    assert!(minimum_owned_bytes > MAX_SHAPED_RUN_BYTES && minimum_owned_bytes < REFERENCE_BYTES);
}
