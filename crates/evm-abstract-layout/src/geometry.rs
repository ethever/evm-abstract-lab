//! Renderer-independent geometry in graph coordinates.

/// A coordinate in the layout's world space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    /// Horizontal coordinate.
    pub x: f64,
    /// Vertical coordinate.
    pub y: f64,
}

impl Point {
    /// Construct a point.
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Translate a local coordinate into its parent coordinate space.
    pub fn translated(self, offset: Self) -> Self {
        Self::new(self.x + offset.x, self.y + offset.y)
    }

    /// Whether both coordinates are finite.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// Measured card or label dimensions, independent of camera scale.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    /// Horizontal extent.
    pub width: f64,
    /// Vertical extent.
    pub height: f64,
}

impl Size {
    /// Construct a size.
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    /// Whether this describes a finite, nonempty rectangle.
    pub fn is_valid(self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }
}

/// A rectangle in graph coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Top-left corner.
    pub origin: Point,
    /// Rectangle dimensions.
    pub size: Size,
}

impl Rect {
    /// Construct a rectangle.
    pub const fn new(origin: Point, size: Size) -> Self {
        Self { origin, size }
    }

    /// Right edge.
    pub fn right(self) -> f64 {
        self.origin.x + self.size.width
    }

    /// Bottom edge.
    pub fn bottom(self) -> f64 {
        self.origin.y + self.size.height
    }

    /// Whether the rectangle is finite and nonempty.
    pub fn is_valid(self) -> bool {
        self.origin.is_finite() && self.size.is_valid()
    }

    /// Smallest rectangle containing both inputs.
    pub fn union(self, other: Self) -> Self {
        let x = self.origin.x.min(other.origin.x);
        let y = self.origin.y.min(other.origin.y);
        Self::new(
            Point::new(x, y),
            Size::new(
                self.right().max(other.right()) - x,
                self.bottom().max(other.bottom()) - y,
            ),
        )
    }
}
