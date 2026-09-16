//! Rendering — stored vs useful (design/language.md §6.7).

/// The exact decimal expansion of a binary64 value: `1.0` → `1`, `-0.0` → `-0`.
pub fn bin64_stored(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf".to_string() } else { "-inf".to_string() };
    }
    // 1074 fractional digits suffice for the smallest subnormal binary64; Rust's
    // fixed-precision formatting is exact.
    trim_fraction(format!("{:.1100}", v))
}

/// The exact decimal expansion of a binary32 value.
pub fn bin32_stored(v: f32) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf".to_string() } else { "-inf".to_string() };
    }
    // 149 fractional digits suffice for the smallest subnormal binary32.
    trim_fraction(format!("{:.200}", v))
}

fn trim_fraction(mut s: String) -> String {
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

/// The useful form: Rust's `{}` at the value's own width.
pub fn bin64_useful(v: f64) -> String {
    format!("{}", v)
}

pub fn bin32_useful(v: f32) -> String {
    format!("{}", v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_forms() {
        assert_eq!(bin32_stored(0.1f32), "0.100000001490116119384765625");
        assert_eq!(bin64_stored(1.0), "1");
        assert_eq!(bin64_stored(-0.0), "-0");
        assert_eq!(bin64_stored(f64::INFINITY), "inf");
        assert_eq!(bin64_stored(f64::NEG_INFINITY), "-inf");
        assert_eq!(bin64_stored(f64::NAN), "NaN");
        assert_eq!(bin64_stored(0.5), "0.5");
        assert_eq!(bin64_stored(1e21), "1000000000000000000000");
    }

    #[test]
    fn useful_forms() {
        assert_eq!(bin32_useful(0.1f32), "0.1");
        assert_eq!(bin64_useful(1.0), "1");
        assert_eq!(bin64_useful(1e21), "1000000000000000000000");
        assert_eq!(bin64_useful(-0.0), "-0");
    }
}
