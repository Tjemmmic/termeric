//! Image storage for the graphics system
//!
//! Stores decoded image data and manages memory limits.

use std::collections::HashMap;

/// Unique identifier for a stored image
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageId(pub u32);

/// A stored image with decoded RGBA data
#[derive(Debug, Clone)]
pub struct StoredImage {
    /// Unique ID
    pub id: ImageId,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// RGBA pixel data
    pub data: Vec<u8>,
}

impl StoredImage {
    /// Get the memory size of this image
    pub fn memory_size(&self) -> usize {
        self.data.len()
    }
}

/// Image placement on the terminal grid
#[derive(Debug, Clone)]
pub struct ImagePlacement {
    /// Image ID this placement refers to
    pub image_id: ImageId,
    /// Optional placement ID for reference
    pub placement_id: Option<u32>,
    /// Column position (0-indexed)
    pub col: u32,
    /// Row position (0-indexed)
    pub row: u32,
    /// X offset within cell (pixels)
    pub x_offset: u32,
    /// Y offset within cell (pixels)
    pub y_offset: u32,
    /// Display width (in columns, if specified)
    pub width: Option<u32>,
    /// Display height (in rows, if specified)
    pub height: Option<u32>,
    /// Z-index for layering
    pub z_index: i32,
}

/// Storage for images with memory management
#[derive(Debug)]
pub struct ImageStorage {
    /// Stored images by ID
    images: HashMap<ImageId, StoredImage>,
    /// Maximum number of images
    max_images: usize,
    /// Maximum total memory
    max_memory: usize,
    /// Current memory usage
    current_memory: usize,
    /// Access order for LRU eviction (most recently accessed at end)
    access_order: Vec<ImageId>,
}

impl ImageStorage {
    /// Create a new image storage with limits
    pub fn new(max_images: usize, max_memory: usize) -> Self {
        Self {
            images: HashMap::new(),
            max_images,
            max_memory,
            current_memory: 0,
            access_order: Vec::new(),
        }
    }

    /// Store an image, evicting old images if necessary
    pub fn store(&mut self, image: StoredImage) -> Result<(), String> {
        let new_size = image.memory_size();

        // Check if single image exceeds memory limit
        if new_size > self.max_memory {
            return Err(format!(
                "Image too large: {} bytes exceeds {} byte limit",
                new_size, self.max_memory
            ));
        }

        // Remove existing image with same ID if present
        if self.images.contains_key(&image.id) {
            self.remove(image.id);
        }

        // Evict images until we have space
        while self.current_memory + new_size > self.max_memory
            || self.images.len() >= self.max_images
        {
            if let Some(oldest_id) = self.access_order.first().copied() {
                self.remove(oldest_id);
            } else {
                break;
            }
        }

        // Store the new image
        self.current_memory += new_size;
        self.access_order.push(image.id);
        self.images.insert(image.id, image);

        Ok(())
    }

    /// Get an image by ID (updates access order)
    pub fn get(&self, id: ImageId) -> Option<&StoredImage> {
        self.images.get(&id)
    }

    /// Get an image mutably
    pub fn get_mut(&mut self, id: ImageId) -> Option<&mut StoredImage> {
        // Update access order
        if let Some(pos) = self.access_order.iter().position(|&i| i == id) {
            self.access_order.remove(pos);
            self.access_order.push(id);
        }
        self.images.get_mut(&id)
    }

    /// Check if an image exists
    pub fn contains(&self, id: ImageId) -> bool {
        self.images.contains_key(&id)
    }

    /// Remove an image
    pub fn remove(&mut self, id: ImageId) {
        if let Some(image) = self.images.remove(&id) {
            self.current_memory = self.current_memory.saturating_sub(image.memory_size());
            self.access_order.retain(|&i| i != id);
        }
    }

    /// Clear all images
    pub fn clear(&mut self) {
        self.images.clear();
        self.access_order.clear();
        self.current_memory = 0;
    }

    /// Get current memory usage
    pub fn memory_usage(&self) -> usize {
        self.current_memory
    }

    /// Get number of stored images
    pub fn len(&self) -> usize {
        self.images.len()
    }

    /// Check if storage is empty
    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// Iterate over all stored images
    pub fn iter(&self) -> impl Iterator<Item = &StoredImage> {
        self.images.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_image(id: u32, size: usize) -> StoredImage {
        StoredImage {
            id: ImageId(id),
            width: 10,
            height: 10,
            data: vec![0u8; size],
        }
    }

    #[test]
    fn test_store_and_retrieve() {
        let mut storage = ImageStorage::new(10, 1024 * 1024);
        let image = create_test_image(1, 100);

        storage.store(image.clone()).unwrap();
        assert!(storage.contains(ImageId(1)));
        assert_eq!(storage.get(ImageId(1)).unwrap().id, ImageId(1));
    }

    #[test]
    fn test_memory_limit_eviction() {
        let mut storage = ImageStorage::new(10, 200);

        // Store first image (100 bytes)
        storage.store(create_test_image(1, 100)).unwrap();
        assert!(storage.contains(ImageId(1)));

        // Store second image (100 bytes) - fits
        storage.store(create_test_image(2, 100)).unwrap();
        assert!(storage.contains(ImageId(1)));
        assert!(storage.contains(ImageId(2)));

        // Store third image (100 bytes) - should evict first
        storage.store(create_test_image(3, 100)).unwrap();
        assert!(!storage.contains(ImageId(1))); // Evicted
        assert!(storage.contains(ImageId(2)));
        assert!(storage.contains(ImageId(3)));
    }

    #[test]
    fn test_count_limit_eviction() {
        let mut storage = ImageStorage::new(2, 1024 * 1024);

        storage.store(create_test_image(1, 10)).unwrap();
        storage.store(create_test_image(2, 10)).unwrap();
        assert_eq!(storage.len(), 2);

        // Third image should evict first
        storage.store(create_test_image(3, 10)).unwrap();
        assert_eq!(storage.len(), 2);
        assert!(!storage.contains(ImageId(1)));
        assert!(storage.contains(ImageId(2)));
        assert!(storage.contains(ImageId(3)));
    }

    #[test]
    fn test_replace_existing() {
        let mut storage = ImageStorage::new(10, 1024);

        storage.store(create_test_image(1, 100)).unwrap();
        assert_eq!(storage.memory_usage(), 100);

        // Replace with larger image
        storage.store(create_test_image(1, 200)).unwrap();
        assert_eq!(storage.len(), 1);
        assert_eq!(storage.memory_usage(), 200);
    }

    #[test]
    fn test_image_too_large() {
        let mut storage = ImageStorage::new(10, 100);
        let result = storage.store(create_test_image(1, 200));
        assert!(result.is_err());
    }
}
