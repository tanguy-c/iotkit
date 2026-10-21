//! Battery charge from its voltage (the ADC reading is `iotkit::battery`).

/// A battery's discharge curve: (millivolts, percent) points, ascending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatteryCurve {
    points: &'static [(u16, u8)],
}

impl BatteryCurve {
    /// One Li-ion/LiPo cell: 3.3 V empty, 4.2 V full.
    pub const LIPO: Self = Self::custom(&[
        (3300, 0),
        (3500, 5),
        (3600, 10),
        (3700, 20),
        (3750, 30),
        (3800, 40),
        (3850, 50),
        (3900, 60),
        (3950, 70),
        (4000, 80),
        (4100, 90),
        (4200, 100),
    ]);

    /// Two alkaline AA cells in series: 2.0 V empty, 3.1 V full.
    pub const ALKALINE_2AA: Self =
        Self::custom(&[(2000, 0), (2200, 8), (2400, 20), (2600, 45), (2800, 70), (2900, 85), (3100, 100)]);

    /// Fails the build unless the points rise in voltage and do not fall in percent.
    pub const fn custom(points: &'static [(u16, u8)]) -> Self {
        assert!(!points.is_empty(), "BatteryCurve: needs at least one point");
        let mut i = 0;
        while i < points.len() {
            assert!(points[i].1 <= 100, "BatteryCurve: percentages are 0..=100");
            if i > 0 {
                assert!(
                    points[i].0 > points[i - 1].0 && points[i].1 >= points[i - 1].1,
                    "BatteryCurve: points must be ascending"
                );
            }
            i += 1;
        }
        Self { points }
    }

    /// The charge at `volts`, interpolated between the points and clamped to the ends.
    pub fn percent(&self, volts: f32) -> f32 {
        if volts.is_nan() {
            return f32::NAN;
        }
        let mv = volts * 1000.0;
        let (first, last) = (self.points[0], self.points[self.points.len() - 1]);
        if mv <= f32::from(first.0) {
            return first.1.into();
        }
        if mv >= f32::from(last.0) {
            return last.1.into();
        }
        let upper = self.points.iter().position(|p| f32::from(p.0) >= mv).unwrap_or(self.points.len() - 1);
        let ((v0, p0), (v1, p1)) = (self.points[upper - 1], self.points[upper]);
        let t = (mv - f32::from(v0)) / f32::from(v1 - v0);
        f32::from(p0) + t * (f32::from(p1) - f32::from(p0))
    }
}

/// The battery's voltage from the ADC pin's, behind a divider of `ratio` (2.0 for two equal
/// resistors).
pub fn divided(pin_millivolts: u16, ratio: f32) -> f32 {
    f32::from(pin_millivolts) * ratio / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_and_clamps() {
        let c = BatteryCurve::LIPO;
        assert_eq!(c.percent(4.3), 100.0);
        assert_eq!(c.percent(3.0), 0.0);
        assert_eq!(c.percent(3.85), 50.0);
        assert!((c.percent(3.725) - 25.0).abs() < 0.01);
        assert!(c.percent(f32::NAN).is_nan());
        assert!((BatteryCurve::ALKALINE_2AA.percent(2.5) - 32.5).abs() < 0.01);
        assert_eq!(divided(1900, 2.0), 3.8);
        const ONE: BatteryCurve = BatteryCurve::custom(&[(3000, 50)]);
        assert_eq!(ONE.percent(3.5), 50.0);
    }
}
