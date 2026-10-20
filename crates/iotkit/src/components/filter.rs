//! Filters for sensor readings, chained with [`Filter::then`]. A closure
//! `|v: f32| -> Option<f32>` is a filter too; `None` drops the reading.

/// Turns a reading into the value to report, or drops it (`None`).
pub trait Filter {
    /// The value to report for `value`, if any.
    fn apply(&mut self, value: f32) -> Option<f32>;

    /// `self`, then `next`.
    fn then<F: Filter>(self, next: F) -> Chain<Self, F>
    where
        Self: Sized,
    {
        Chain(self, next)
    }
}

impl<F: FnMut(f32) -> Option<f32>> Filter for F {
    fn apply(&mut self, value: f32) -> Option<f32> {
        self(value)
    }
}

/// Two filters in a row ([`Filter::then`]).
pub struct Chain<A, B>(A, B);

impl<A: Filter, B: Filter> Filter for Chain<A, B> {
    fn apply(&mut self, value: f32) -> Option<f32> {
        self.0.apply(value).and_then(|v| self.1.apply(v))
    }
}

/// Adds a constant (a calibration offset).
pub struct Offset(pub f32);

impl Filter for Offset {
    fn apply(&mut self, value: f32) -> Option<f32> {
        Some(value + self.0)
    }
}

/// Multiplies by a constant (a unit change).
pub struct Multiply(pub f32);

impl Filter for Multiply {
    fn apply(&mut self, value: f32) -> Option<f32> {
        Some(value * self.0)
    }
}

/// Keeps the value within `min..=max`.
pub struct Clamp {
    min: f32,
    max: f32,
}

impl Clamp {
    /// Within `min..=max`.
    pub const fn new(min: f32, max: f32) -> Self {
        assert!(min <= max, "Clamp: min > max");
        Self { min, max }
    }
}

impl Filter for Clamp {
    fn apply(&mut self, value: f32) -> Option<f32> {
        Some(value.clamp(self.min, self.max))
    }
}

/// The straight line through two calibration points: `(raw, value)`.
pub struct Linear {
    slope: f32,
    intercept: f32,
}

impl Linear {
    /// Maps `a.0` to `a.1` and `b.0` to `b.1` (and everything in between, or beyond, on the line).
    pub const fn new(a: (f32, f32), b: (f32, f32)) -> Self {
        assert!(a.0 != b.0, "Linear: two different raw values");
        let slope = (b.1 - a.1) / (b.0 - a.0);
        Self { slope, intercept: a.1 - slope * a.0 }
    }
}

impl Filter for Linear {
    fn apply(&mut self, value: f32) -> Option<f32> {
        Some(value * self.slope + self.intercept)
    }
}

/// The mean of the last `N` readings (the mean so far until there are `N`).
pub struct MovingAverage<const N: usize> {
    values: [f32; N],
    len: usize,
    next: usize,
}

impl<const N: usize> MovingAverage<N> {
    /// No readings yet.
    pub const fn new() -> Self {
        assert!(N > 0, "MovingAverage: N > 0");
        Self { values: [0.0; N], len: 0, next: 0 }
    }
}

impl<const N: usize> Default for MovingAverage<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Filter for MovingAverage<N> {
    fn apply(&mut self, value: f32) -> Option<f32> {
        self.values[self.next] = value;
        self.next = (self.next + 1) % N;
        self.len = (self.len + 1).min(N);
        Some(self.values[..self.len].iter().sum::<f32>() / self.len as f32)
    }
}

/// Drops a reading that differs by less than `min` from the last one reported.
pub struct Delta {
    min: f32,
    last: Option<f32>,
}

impl Delta {
    /// Reports changes of at least `min`.
    pub const fn new(min: f32) -> Self {
        Self { min, last: None }
    }
}

impl Filter for Delta {
    fn apply(&mut self, value: f32) -> Option<f32> {
        if self.last.is_some_and(|last| (value - last).abs() < self.min) {
            return None;
        }
        self.last = Some(value);
        Some(value)
    }
}
