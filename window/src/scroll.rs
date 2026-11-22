const SCROLL: &str = include_str!("scroll.js");

pub fn page_script(nonce: &str) -> String {
    SCROLL.replacen(
        "NONCE",
        &serde_json::to_string(nonce).unwrap_or_default(),
        1,
    )
}

pub fn over_slider(nonce: &str, message: &str) -> Option<bool> {
    let rest = message
        .trim()
        .strip_prefix(nonce)?
        .strip_prefix(":slider:")?;
    match rest {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_the_window_nonce_into_the_scroll_script() {
        let script = page_script("soundscape123");
        assert!(script.contains("\"soundscape123\""));
        assert!(script.contains("soundscape-scrollbar"));
    }

    #[test]
    fn reads_when_the_pointer_is_over_a_slider() {
        assert_eq!(over_slider("abc", "abc:slider:1"), Some(true));
        assert_eq!(over_slider("abc", "abc:slider:0"), Some(false));
        assert_eq!(over_slider("abc", "other:slider:1"), None);
        assert_eq!(over_slider("abc", "abc:slider:maybe"), None);
    }
}
