use std::time::Duration;

pub const DEFAULT_LIMIT_MB: u32 = 500;
pub const LIMITS_MB: [u32; 4] = [0, 400, 500, 700];
pub const SETTLE: Duration = Duration::from_secs(60);
const PAGE_BYTES: u64 = 4096;

pub fn resident_mb(statm: &str) -> Option<u32> {
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some((pages * PAGE_BYTES / 1024 / 1024) as u32)
}

pub fn own_resident_mb() -> Option<u32> {
    resident_mb(&std::fs::read_to_string("/proc/self/statm").ok()?)
}

pub fn should_arm(limit_mb: u32, resident_mb: Option<u32>, since_load: Duration) -> bool {
    limit_mb > 0 && since_load >= SETTLE && resident_mb.is_some_and(|used| used > limit_mb)
}

pub fn allowed(limit_mb: u32) -> u32 {
    if LIMITS_MB.contains(&limit_mb) {
        limit_mb
    } else {
        DEFAULT_LIMIT_MB
    }
}

pub fn flag_script(armed: bool) -> String {
    format!("window.soundscapeFreshLoad = {armed};")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_its_own_memory() {
        assert_eq!(resident_mb("1000000 153600 2000 1 0 3000 0"), Some(600));
        assert_eq!(resident_mb("nonsense"), None);
        assert_eq!(resident_mb(""), None);
    }

    #[test]
    fn opens_only_above_the_limit_after_settling() {
        let settled = SETTLE + Duration::from_secs(1);
        assert!(should_arm(500, Some(600), settled));
        assert!(!should_arm(500, Some(400), settled));
        assert!(!should_arm(500, Some(600), Duration::from_secs(5)));
        assert!(!should_arm(0, Some(900), settled));
        assert!(!should_arm(500, None, settled));
    }

    #[test]
    fn keeps_to_the_offered_limits() {
        assert_eq!(allowed(400), 400);
        assert_eq!(allowed(0), 0);
        assert_eq!(allowed(123), DEFAULT_LIMIT_MB);
        assert_eq!(flag_script(true), "window.soundscapeFreshLoad = true;");
    }
}
