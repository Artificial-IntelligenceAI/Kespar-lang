//! Bin rendering (language.md §6.7): the stored form is the exact decimal
//! expansion; the useful form is the shortest round-trip decimal.

pub fn stored64(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    // Rust prints the exact value at any precision; 1074 fractional digits
    // covers the smallest subnormal.
    trim(format!("{f:.1074}"))
}

pub fn stored32(f: f32) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    trim(format!("{f:.149}"))
}

fn trim(mut s: String) -> String {
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

pub fn useful64(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    format!("{f}")
}

pub fn useful32(f: f32) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    format!("{f}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forms() {
        assert_eq!(stored32(0.1), "0.100000001490116119384765625");
        assert_eq!(stored64(1.0), "1");
        assert_eq!(stored64(-0.0), "-0");
        assert_eq!(useful32(0.1), "0.1");
        assert_eq!(useful64(1e21), "1000000000000000000000");
        assert_eq!(useful64(f64::INFINITY), "inf");
    }
}
