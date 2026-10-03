//! Rectangles in physical window pixels, shared by layout, hit testing,
//! drawing, and accessibility.

/// A rectangle in physical window pixels. Its left and top edges are inside
/// it; its right and bottom edges belong to whatever is next.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Bounds {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(self) -> f32 {
        self.y + self.height
    }

    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn inset(self, horizontal: f32, vertical: f32) -> Self {
        Self::new(
            self.x + horizontal,
            self.y + vertical,
            (self.width - horizontal * 2.0).max(0.0),
            (self.height - vertical * 2.0).max(0.0),
        )
    }

    pub fn intersect(self, other: Bounds) -> Option<Bounds> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > x && bottom > y).then(|| Bounds::new(x, y, right - x, bottom - y))
    }

    pub fn center(self) -> [f32; 2] {
        [self.x + self.width / 2.0, self.y + self.height / 2.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_is_half_open() {
        let bounds = Bounds::new(10.0, 20.0, 30.0, 40.0);
        assert!(bounds.contains(10.0, 20.0));
        assert!(bounds.contains(39.9, 59.9));
        assert!(!bounds.contains(40.0, 30.0));
        assert!(!bounds.contains(20.0, 60.0));
        assert!(!bounds.contains(9.9, 30.0));
    }

    #[test]
    fn bounds_intersect_and_inset() {
        let a = Bounds::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(
            a.intersect(Bounds::new(5.0, 5.0, 10.0, 10.0)),
            Some(Bounds::new(5.0, 5.0, 5.0, 5.0))
        );
        assert_eq!(a.intersect(Bounds::new(10.0, 0.0, 5.0, 5.0)), None);
        assert_eq!(a.inset(2.0, 3.0), Bounds::new(2.0, 3.0, 6.0, 4.0));
        assert_eq!(a.inset(20.0, 0.0).width, 0.0);
    }
}
