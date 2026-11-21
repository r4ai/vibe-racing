use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// Wrap a lap fraction difference into (-0.5, 0.5].
pub(crate) fn wrap_half(d: f64) -> f64 {
    let mut d = d % 1.0;
    if d > 0.5 {
        d -= 1.0;
    } else if d <= -0.5 {
        d += 1.0;
    }
    d
}

pub(crate) fn mean(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        None
    } else {
        Some(xs.iter().sum::<f64>() / xs.len() as f64)
    }
}

/// Lap times / fuel are only meaningful when strictly positive.
pub(crate) fn positive(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite() && *x > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_half_range_is_half_open() {
        assert_eq!(wrap_half(0.5), 0.5);
        assert_eq!(wrap_half(-0.5), 0.5);
        assert!((wrap_half(1.7) - (-0.3)).abs() < 1e-9);
        assert!((wrap_half(-1.2) - (-0.2)).abs() < 1e-9);
        assert_eq!(wrap_half(0.0), 0.0);
    }

    #[test]
    fn now_is_wall_clock() {
        // 2024-01-01 in ms; any real clock is past it.
        assert!(now_ms() > 1_704_067_200_000);
    }

    #[test]
    fn helpers() {
        assert_eq!(mean(&[]), None);
        assert_eq!(mean(&[1.0, 2.0, 6.0]), Some(3.0));
        assert_eq!(positive(Some(0.0)), None);
        assert_eq!(positive(Some(f64::NAN)), None);
        assert_eq!(positive(Some(f64::INFINITY)), None);
        assert_eq!(positive(Some(0.1)), Some(0.1));
    }

    #[test]
    fn wrap_half_wraps_both_ways() {
        assert!((wrap_half(0.9) - (-0.1)).abs() < 1e-9);
        assert!((wrap_half(-0.9) - 0.1).abs() < 1e-9);
        assert!((wrap_half(0.2) - 0.2).abs() < 1e-9);
    }
}
