//! Human-readable byte counts.

/// Format a byte count as human-readable (base 1024, "B/KB/MB/GB/TB/PB").
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_through_the_units() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(1024u64.pow(2)), "1.0 MB");
        assert_eq!(human_size(1024u64.pow(5)), "1.0 PB");
        // Stays at the top unit beyond PB.
        assert_eq!(human_size(2 * 1024u64.pow(5)), "2.0 PB");
    }
}
