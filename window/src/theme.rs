use servo::Theme;
use soundscape_protocol::theme::{backdrop_rgba, chosen_dark};

pub fn from_dark(dark: bool) -> Theme {
    if dark { Theme::Dark } else { Theme::Light }
}

pub fn chosen(value: Option<&str>) -> Option<Theme> {
    chosen_dark(value).map(from_dark)
}

pub fn backdrop(theme: Theme) -> [f64; 4] {
    backdrop_rgba(theme == Theme::Dark)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundscape_protocol::theme::from_scheme_dark;

    #[test]
    fn follows_the_desktop_colour_scheme() {
        assert_eq!(from_dark(from_scheme_dark(1)), Theme::Dark);
        assert_eq!(from_dark(from_scheme_dark(2)), Theme::Light);
        assert_eq!(from_dark(from_scheme_dark(0)), Theme::Light);
        assert_eq!(chosen(Some("dark")), Some(Theme::Dark));
        assert_eq!(chosen(Some("light")), Some(Theme::Light));
        assert_eq!(chosen(Some("blue")), None);
        assert_eq!(backdrop(Theme::Light), [1.0, 1.0, 1.0, 1.0]);
        assert!(backdrop(Theme::Dark)[0] < 0.1);
    }
}
