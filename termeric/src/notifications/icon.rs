//! Bounded decoding and process-lifetime caching for OSC 99 notification icons.

use image::{ImageFormat, ImageReader, Limits};
use std::collections::{HashMap, VecDeque};
use std::io::Cursor;
use std::sync::{Arc, Mutex, OnceLock};

pub(super) const MAX_ICON_DIMENSION: u32 = 512;
pub(super) const MAX_ICON_DECODED_BYTES: u64 =
    MAX_ICON_DIMENSION as u64 * MAX_ICON_DIMENSION as u64 * 4;
pub(super) const MAX_ICON_CACHE_ENTRIES: usize = 64;
pub(super) const MAX_ICON_CACHE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DecodedNotificationIcon {
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) rowstride: i32,
    pub(super) data: Vec<u8>,
}

fn decode_icon(encoded: &[u8]) -> Result<DecodedNotificationIcon, String> {
    if encoded.is_empty() || encoded.len() > termeric_core::term::MAX_NOTIFICATION_ICON_BYTES {
        return Err("transmitted notification icon exceeds its encoded-byte limit".into());
    }
    let mut reader = ImageReader::new(Cursor::new(encoded))
        .with_guessed_format()
        .map_err(|error| format!("could not identify transmitted notification icon: {error}"))?;
    let format = reader
        .format()
        .ok_or_else(|| "transmitted notification icon has no recognized format".to_string())?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif
    ) {
        return Err(format!(
            "unsupported transmitted notification icon format {format:?}"
        ));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_ICON_DIMENSION);
    limits.max_image_height = Some(MAX_ICON_DIMENSION);
    limits.max_alloc = Some(MAX_ICON_DECODED_BYTES * 2);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|error| format!("could not decode transmitted notification icon: {error}"))?;
    let width = decoded.width();
    let height = decoded.height();
    let rowstride = width
        .checked_mul(4)
        .ok_or_else(|| "transmitted notification icon rowstride overflowed".to_string())?;
    let expected = u64::from(rowstride)
        .checked_mul(u64::from(height))
        .ok_or_else(|| "transmitted notification icon byte length overflowed".to_string())?;
    if expected > MAX_ICON_DECODED_BYTES {
        return Err("transmitted notification icon exceeds its decoded-byte limit".into());
    }
    let data = decoded.into_rgba8().into_raw();
    if data.len() as u64 != expected {
        return Err("transmitted notification icon produced inconsistent pixels".into());
    }
    Ok(DecodedNotificationIcon {
        width: i32::try_from(width).map_err(|_| "icon width is not representable")?,
        height: i32::try_from(height).map_err(|_| "icon height is not representable")?,
        rowstride: i32::try_from(rowstride).map_err(|_| "icon rowstride is not representable")?,
        data,
    })
}

#[derive(Default)]
pub(super) struct NotificationIconCache {
    entries: HashMap<String, Arc<DecodedNotificationIcon>>,
    order: VecDeque<String>,
    retained_bytes: usize,
}

impl NotificationIconCache {
    pub(super) fn resolve(
        &mut self,
        cache_id: Option<&str>,
        fresh: Option<&[u8]>,
    ) -> (Option<Arc<DecodedNotificationIcon>>, Option<String>) {
        if let Some(encoded) = fresh {
            match decode_icon(encoded) {
                Ok(icon) => {
                    let icon = Arc::new(icon);
                    if let Some(cache_id) = cache_id {
                        self.insert(cache_id.to_owned(), Arc::clone(&icon));
                    }
                    return (Some(icon), None);
                }
                Err(error) => {
                    let previous = cache_id.and_then(|cache_id| self.get(cache_id));
                    return (previous, Some(error));
                }
            }
        }
        let icon = cache_id.and_then(|cache_id| self.get(cache_id));
        let warning = (cache_id.is_some() && icon.is_none())
            .then(|| "transmitted notification icon cache entry is unavailable".into());
        (icon, warning)
    }

    fn get(&mut self, cache_id: &str) -> Option<Arc<DecodedNotificationIcon>> {
        let icon = self.entries.get(cache_id).cloned()?;
        self.order.retain(|candidate| candidate != cache_id);
        self.order.push_back(cache_id.to_owned());
        Some(icon)
    }

    fn insert(&mut self, cache_id: String, icon: Arc<DecodedNotificationIcon>) {
        if let Some(previous) = self.entries.remove(&cache_id) {
            self.retained_bytes = self.retained_bytes.saturating_sub(previous.data.len());
        }
        self.order.retain(|candidate| candidate != &cache_id);
        self.retained_bytes = self.retained_bytes.saturating_add(icon.data.len());
        self.entries.insert(cache_id.clone(), icon);
        self.order.push_back(cache_id);
        while self.entries.len() > MAX_ICON_CACHE_ENTRIES
            || self.retained_bytes > MAX_ICON_CACHE_BYTES
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(icon) = self.entries.remove(&oldest) {
                self.retained_bytes = self.retained_bytes.saturating_sub(icon.data.len());
            }
        }
    }
}

static ICON_CACHE: OnceLock<Mutex<NotificationIconCache>> = OnceLock::new();

pub(super) fn resolve_transmitted_icon(
    cache_id: Option<&str>,
    fresh: Option<&[u8]>,
) -> (Option<Arc<DecodedNotificationIcon>>, Option<String>) {
    match ICON_CACHE
        .get_or_init(|| Mutex::new(NotificationIconCache::default()))
        .lock()
    {
        Ok(mut cache) => cache.resolve(cache_id, fresh),
        Err(_) => (
            None,
            Some("transmitted notification icon cache is unavailable".into()),
        ),
    }
}

pub(super) fn first_available_icon_name(names: &[String]) -> Option<String> {
    first_available_icon_name_with(names, |mapped| {
        freedesktop_icons::lookup(mapped)
            .with_size(256)
            .find()
            .is_some()
    })
}

pub(super) fn is_safe_icon_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains('/') && !name.contains('\\')
}

fn first_available_icon_name_with(
    names: &[String],
    mut available: impl FnMut(&str) -> bool,
) -> Option<String> {
    names
        .iter()
        .filter(|name| is_safe_icon_name(name))
        .find_map(|name| {
            let mapped = super::freedesktop_icon_name(name);
            available(mapped).then(|| mapped.to_owned())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba(color));
        let mut encoded = Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, ImageFormat::Png)
            .expect("in-memory PNG should encode");
        encoded.into_inner()
    }

    #[test]
    fn decoder_produces_consistent_bounded_rgba() {
        let decoded = decode_icon(&png(2, 3, [1, 2, 3, 4])).unwrap();
        assert_eq!(
            (decoded.width, decoded.height, decoded.rowstride),
            (2, 3, 8)
        );
        assert_eq!(decoded.data.len(), 24);
        assert!(decode_icon(b"not an image").is_err());
        assert!(decode_icon(&png(MAX_ICON_DIMENSION + 1, 1, [0; 4])).is_err());
        for format in [ImageFormat::Jpeg, ImageFormat::Gif] {
            let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                2,
                1,
                image::Rgb([10, 20, 30]),
            ));
            let mut encoded = Cursor::new(Vec::new());
            image.write_to(&mut encoded, format).unwrap();
            let decoded = decode_icon(&encoded.into_inner()).unwrap();
            assert_eq!(
                (decoded.width, decoded.height, decoded.rowstride),
                (2, 1, 8)
            );
        }

        let mut animated = Cursor::new(Vec::new());
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut animated);
            encoder
                .encode_frames([
                    image::Frame::new(image::RgbaImage::from_pixel(
                        1,
                        1,
                        image::Rgba([1, 2, 3, 255]),
                    )),
                    image::Frame::new(image::RgbaImage::from_pixel(
                        1,
                        1,
                        image::Rgba([5, 6, 7, 255]),
                    )),
                ])
                .unwrap();
        }
        assert_eq!(
            decode_icon(&animated.into_inner()).unwrap().data,
            [1, 2, 3, 255]
        );
    }

    #[test]
    fn cache_reuses_replaces_and_preserves_previous_on_invalid_data() {
        let mut cache = NotificationIconCache::default();
        let first = png(1, 1, [1, 2, 3, 4]);
        let second = png(1, 1, [5, 6, 7, 8]);
        assert!(cache.resolve(Some("shared"), Some(&first)).1.is_none());
        assert_eq!(
            cache.resolve(Some("shared"), None).0.unwrap().data,
            [1, 2, 3, 4]
        );
        assert!(cache.resolve(Some("shared"), Some(b"bad")).1.is_some());
        assert_eq!(
            cache.resolve(Some("shared"), None).0.unwrap().data,
            [1, 2, 3, 4]
        );
        assert!(cache.resolve(Some("shared"), Some(&second)).1.is_none());
        assert_eq!(
            cache.resolve(Some("shared"), None).0.unwrap().data,
            [5, 6, 7, 8]
        );
        assert!(cache.resolve(Some("missing"), None).1.is_some());
    }

    #[test]
    fn cache_entry_pressure_evicts_least_recently_used() {
        let mut cache = NotificationIconCache::default();
        let icon = png(1, 1, [1, 2, 3, 4]);
        for index in 0..MAX_ICON_CACHE_ENTRIES {
            cache.resolve(Some(&format!("icon-{index}")), Some(&icon));
        }
        cache.resolve(Some("icon-0"), None);
        cache.resolve(Some("newest"), Some(&icon));
        assert!(cache.resolve(Some("icon-0"), None).0.is_some());
        assert!(cache.resolve(Some("icon-1"), None).0.is_none());
        assert!(cache.retained_bytes <= MAX_ICON_CACHE_BYTES);
    }

    #[test]
    fn cache_byte_pressure_is_bounded_and_deterministic() {
        let mut cache = NotificationIconCache::default();
        let bytes_per_icon = MAX_ICON_DECODED_BYTES as usize;
        for index in 0..=MAX_ICON_CACHE_BYTES / bytes_per_icon {
            cache.insert(
                format!("large-{index}"),
                Arc::new(DecodedNotificationIcon {
                    width: MAX_ICON_DIMENSION as i32,
                    height: MAX_ICON_DIMENSION as i32,
                    rowstride: (MAX_ICON_DIMENSION * 4) as i32,
                    data: vec![0; bytes_per_icon],
                }),
            );
        }
        assert!(cache.retained_bytes <= MAX_ICON_CACHE_BYTES);
        assert!(!cache.entries.contains_key("large-0"));
    }

    #[test]
    fn local_icon_names_keep_protocol_order_and_universal_aliases() {
        let names = vec![
            "/tmp/untrusted.png".to_string(),
            "warning".to_string(),
            "second".to_string(),
        ];
        let mut checked = Vec::new();
        let selected = first_available_icon_name_with(&names, |name| {
            checked.push(name.to_owned());
            name == "second"
        });
        assert_eq!(selected.as_deref(), Some("second"));
        assert_eq!(checked, ["dialog-warning", "second"]);

        let selected = first_available_icon_name_with(&names, |_| true);
        assert_eq!(selected.as_deref(), Some("dialog-warning"));
    }
}
