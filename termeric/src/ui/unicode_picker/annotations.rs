//! Zero-copy access to the generated CLDR English annotation index.

use std::sync::OnceLock;
use unicode_segmentation::UnicodeSegmentation;

pub(super) const CLDR_VERSION: &str = "48.0.0";
pub(super) const CLDR_LOCALE: &str = "en";
pub(super) const CLDR_RECORD_COUNT: usize = 4_342;
pub(super) const CLDR_STATIC_BYTES_BUDGET: usize = 768 * 1024;

const MAGIC: &[u8; 8] = b"TRMCLD01";
const HEADER_BYTES: usize = 16;
const RECORD_BYTES: usize = 16;
const DATA: &[u8] = include_bytes!("cldr_annotations.bin");

static INDEX: OnceLock<AnnotationIndex<'static>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Annotation<'a> {
    pub text: &'a str,
    pub short_name: &'a str,
    pub searchable: &'a str,
}

pub(super) struct AnnotationIndex<'a> {
    records: &'a [u8],
    pool: &'a str,
    static_bytes: usize,
}

impl<'a> AnnotationIndex<'a> {
    fn load(data: &'a [u8]) -> Self {
        assert!(
            data.len() >= HEADER_BYTES,
            "CLDR annotation index is truncated"
        );
        assert_eq!(&data[..MAGIC.len()], MAGIC, "invalid CLDR annotation magic");
        let count = read_u32(data, 8) as usize;
        let pool_bytes = read_u32(data, 12) as usize;
        assert_eq!(count, CLDR_RECORD_COUNT, "unexpected CLDR record count");
        assert!(count <= 5_000, "CLDR record ceiling exceeded");
        let records_bytes = count
            .checked_mul(RECORD_BYTES)
            .expect("CLDR record table overflowed");
        let pool_start = HEADER_BYTES
            .checked_add(records_bytes)
            .expect("CLDR pool offset overflowed");
        let expected_len = pool_start
            .checked_add(pool_bytes)
            .expect("CLDR index length overflowed");
        assert_eq!(data.len(), expected_len, "invalid CLDR annotation length");
        assert!(
            data.len() <= CLDR_STATIC_BYTES_BUDGET,
            "CLDR static byte ceiling exceeded"
        );
        let records = &data[HEADER_BYTES..pool_start];
        let pool =
            std::str::from_utf8(&data[pool_start..]).expect("CLDR annotation pool is not UTF-8");
        let index = Self {
            records,
            pool,
            static_bytes: data.len(),
        };
        index.validate();
        index
    }

    fn validate(&self) {
        let mut previous_end = 0;
        let mut previous_text: Option<&str> = None;
        for position in 0..self.len() {
            let (text_start, name_start, search_start, end) = self.offsets(position);
            assert_eq!(
                text_start, previous_end,
                "CLDR pool contains an unowned gap"
            );
            assert!(
                text_start < name_start && name_start < search_start && search_start < end,
                "CLDR annotation has an empty or unordered field"
            );
            assert!(end <= self.pool.len(), "CLDR annotation exceeds its pool");
            assert!(
                self.pool.is_char_boundary(text_start)
                    && self.pool.is_char_boundary(name_start)
                    && self.pool.is_char_boundary(search_start)
                    && self.pool.is_char_boundary(end),
                "CLDR annotation offset splits UTF-8"
            );
            let annotation = self.annotation(position);
            assert_eq!(
                annotation.text.graphemes(true).count(),
                1,
                "CLDR annotation is not one extended grapheme"
            );
            assert!(
                !annotation.text.chars().any(char::is_control)
                    && !annotation.short_name.chars().any(char::is_control)
                    && !annotation.searchable.chars().any(char::is_control),
                "CLDR annotation contains a control character"
            );
            if let Some(previous) = previous_text {
                assert!(
                    previous < annotation.text,
                    "CLDR annotations are not sorted"
                );
            }
            previous_text = Some(annotation.text);
            previous_end = end;
        }
        assert_eq!(
            previous_end,
            self.pool.len(),
            "CLDR pool has trailing bytes"
        );
    }

    fn offsets(&self, position: usize) -> (usize, usize, usize, usize) {
        let start = position
            .checked_mul(RECORD_BYTES)
            .expect("CLDR record offset overflowed");
        assert!(
            start + RECORD_BYTES <= self.records.len(),
            "CLDR record is absent"
        );
        (
            read_u32(self.records, start) as usize,
            read_u32(self.records, start + 4) as usize,
            read_u32(self.records, start + 8) as usize,
            read_u32(self.records, start + 12) as usize,
        )
    }

    pub(super) fn annotation(&self, position: usize) -> Annotation<'_> {
        let (text_start, name_start, search_start, end) = self.offsets(position);
        Annotation {
            text: &self.pool[text_start..name_start],
            short_name: &self.pool[name_start..search_start],
            searchable: &self.pool[search_start..end],
        }
    }

    pub(super) fn len(&self) -> usize {
        self.records.len() / RECORD_BYTES
    }

    pub(super) fn static_bytes(&self) -> usize {
        self.static_bytes
    }
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        data[offset..offset + 4]
            .try_into()
            .expect("CLDR integer is truncated"),
    )
}

pub(super) fn index() -> &'static AnnotationIndex<'static> {
    INDEX.get_or_init(|| AnnotationIndex::load(DATA))
}

#[cfg(test)]
pub(super) fn validate_embedded_index() -> AnnotationIndex<'static> {
    AnnotationIndex::load(DATA)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_index_is_valid_bounded_and_sequence_aware() {
        let index = validate_embedded_index();
        assert_eq!(index.len(), CLDR_RECORD_COUNT);
        assert_eq!(index.static_bytes(), 475_510);
        assert!(index.static_bytes() <= CLDR_STATIC_BYTES_BUDGET);
        let technologist = (0..index.len())
            .map(|position| index.annotation(position))
            .find(|annotation| annotation.text == "🧑‍💻")
            .unwrap();
        assert_eq!(technologist.short_name, "technologist");
        assert!(technologist.searchable.contains("DEVELOPER"));
    }
}
