//! Hyperlink registry for OSC 8 hyperlinks
//!
//! Manages storage of hyperlink URLs and assigns unique IDs to each hyperlink.

use std::collections::HashMap;

/// A hyperlink entry with URL and optional ID parameter
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hyperlink {
    /// The URL target
    pub url: String,
    /// Optional ID parameter (for grouping hyperlinks across lines)
    pub id: Option<String>,
}

impl Hyperlink {
    /// Create a new hyperlink with just a URL
    pub fn new(url: String) -> Self {
        Self { url, id: None }
    }

    /// Create a new hyperlink with URL and ID
    pub fn with_id(url: String, id: String) -> Self {
        Self { url, id: Some(id) }
    }
}

/// Registry for managing hyperlinks
///
/// Stores hyperlinks and assigns unique numeric IDs for cell references.
/// Uses a simple incrementing counter for IDs.
#[derive(Debug, Default)]
pub struct HyperlinkRegistry {
    /// Map from registry ID to hyperlink
    links: HashMap<u32, Hyperlink>,
    /// Map from (url, id) to registry ID for deduplication
    url_to_id: HashMap<(String, Option<String>), u32>,
    /// Next available ID
    next_id: u32,
}

impl HyperlinkRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Get or create a hyperlink ID for the given URL and optional id parameter
    ///
    /// If a hyperlink with the same URL and ID already exists, returns the existing ID.
    /// Otherwise, creates a new entry and returns the new ID.
    pub fn get_or_insert(&mut self, url: String, id: Option<String>) -> u32 {
        let key = (url.clone(), id.clone());

        if let Some(&existing_id) = self.url_to_id.get(&key) {
            return existing_id;
        }

        let new_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        let hyperlink = match id {
            Some(id_str) => Hyperlink::with_id(url, id_str),
            None => Hyperlink::new(url),
        };

        self.links.insert(new_id, hyperlink);
        self.url_to_id.insert(key, new_id);

        new_id
    }

    /// Get a hyperlink by its ID
    pub fn get(&self, id: u32) -> Option<&Hyperlink> {
        self.links.get(&id)
    }

    /// Get the URL for a hyperlink ID
    pub fn get_url(&self, id: u32) -> Option<&str> {
        self.links.get(&id).map(|h| h.url.as_str())
    }

    /// Clear all hyperlinks
    pub fn clear(&mut self) {
        self.links.clear();
        self.url_to_id.clear();
        // Don't reset next_id to avoid potential confusion with old cell references
    }

    /// Get the number of registered hyperlinks
    pub fn len(&self) -> usize {
        self.links.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hyperlink_creation() {
        let link = Hyperlink::new("https://example.com".to_string());
        assert_eq!(link.url, "https://example.com");
        assert!(link.id.is_none());

        let link_with_id =
            Hyperlink::with_id("https://example.com".to_string(), "my-link".to_string());
        assert_eq!(link_with_id.id, Some("my-link".to_string()));
    }

    #[test]
    fn test_registry_insert() {
        let mut registry = HyperlinkRegistry::new();

        let id1 = registry.get_or_insert("https://example.com".to_string(), None);
        let id2 = registry.get_or_insert("https://other.com".to_string(), None);

        assert_ne!(id1, id2);
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn test_registry_dedup() {
        let mut registry = HyperlinkRegistry::new();

        let id1 = registry.get_or_insert("https://example.com".to_string(), None);
        let id2 = registry.get_or_insert("https://example.com".to_string(), None);

        assert_eq!(id1, id2);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_registry_with_id_param() {
        let mut registry = HyperlinkRegistry::new();

        // Same URL but different IDs should be different entries
        let id1 =
            registry.get_or_insert("https://example.com".to_string(), Some("link1".to_string()));
        let id2 =
            registry.get_or_insert("https://example.com".to_string(), Some("link2".to_string()));

        assert_ne!(id1, id2);
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn test_get_url() {
        let mut registry = HyperlinkRegistry::new();

        let id = registry.get_or_insert("https://example.com".to_string(), None);
        assert_eq!(registry.get_url(id), Some("https://example.com"));
        assert_eq!(registry.get_url(999), None);
    }
}
