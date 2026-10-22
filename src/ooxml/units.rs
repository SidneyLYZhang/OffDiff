//! Unit conversions into the canonical centipoint (1/100 pt).

use crate::model::Cp;

/// Twentieths of a point (twips) as used by WordprocessingML.
pub fn twips(value: i64) -> Cp {
    value * 5
}

/// Half-points as used by `w:sz`.
pub fn half_points(value: i64) -> Cp {
    value * 50
}

/// English Metric Units as used by DrawingML.
pub fn emu(value: i64) -> Cp {
    (value as f64 / 127.0).round() as i64
}

/// PDF points.
pub fn points(value: f64) -> Cp {
    (value * 100.0).round() as i64
}

/// Parse an integer, ignoring surrounding whitespace.
pub fn parse_i64(value: &str) -> Option<i64> {
    value.trim().parse().ok()
}

/// Parse a decimal number, ignoring surrounding whitespace.
pub fn parse_f64(value: &str) -> Option<f64> {
    value.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_common_units() {
        assert_eq!(twips(720), 3600); // 36 pt
        assert_eq!(half_points(44), 2200); // 22 pt
        assert_eq!(emu(12700), 100); // 1 pt
        assert_eq!(points(12.5), 1250);
        assert_eq!(parse_i64(" 42 "), Some(42));
        assert_eq!(parse_f64("1.5"), Some(1.5));
    }
}
