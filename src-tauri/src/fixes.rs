use crate::scrobble::Track;
use serde::{Deserialize, Serialize};

const LONGEST: usize = 300;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Fix {
    pub artist: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub new_artist: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_title: Option<String>,
}

fn same(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

fn clean(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(LONGEST)
        .collect()
}

impl Fix {
    fn matches(&self, track: &Track) -> bool {
        same(&self.artist, &track.artist)
            && self
                .title
                .as_deref()
                .is_none_or(|title| same(title, &track.title))
    }

    fn same_target(&self, other: &Fix) -> bool {
        same(&self.artist, &other.artist)
            && match (&self.title, &other.title) {
                (Some(a), Some(b)) => same(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

pub fn apply(fixes: &[Fix], track: Track) -> Track {
    let fix = fixes
        .iter()
        .find(|fix| fix.title.is_some() && fix.matches(&track))
        .or_else(|| {
            fixes
                .iter()
                .find(|fix| fix.title.is_none() && fix.matches(&track))
        });
    match fix {
        Some(fix) => Track {
            artist: fix.new_artist.clone(),
            title: fix.new_title.clone().unwrap_or(track.title.clone()),
            ..track
        },
        None => track,
    }
}

pub fn make(
    artist: &str,
    title: &str,
    new_artist: &str,
    new_title: &str,
    every_song: bool,
) -> Result<Fix, String> {
    let (artist, title, new_artist, new_title) = (
        clean(artist),
        clean(title),
        clean(new_artist),
        clean(new_title),
    );
    if artist.is_empty()
        || new_artist.is_empty()
        || (!every_song && (title.is_empty() || new_title.is_empty()))
    {
        return Err("Fill in the artist and the title.".into());
    }
    let fix = Fix {
        title: (!every_song).then_some(title),
        new_title: (!every_song).then_some(new_title),
        artist,
        new_artist,
    };
    if fix.artist == fix.new_artist && fix.title == fix.new_title {
        return Err("That's the same as what the player sends.".into());
    }
    Ok(fix)
}

pub fn add(fixes: &mut Vec<Fix>, fix: Fix) {
    fixes.retain(|old| !old.same_target(&fix));
    fixes.push(fix);
}

pub fn describe(fix: &Fix) -> (String, String) {
    match (&fix.title, &fix.new_title) {
        (Some(title), Some(new_title)) => (
            format!("{title} · {}", fix.artist),
            format!("{new_title} · {}", fix.new_artist),
        ),
        _ => (
            format!("Every song by {}", fix.artist),
            format!("by {}", fix.new_artist),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(artist: &str, title: &str) -> Track {
        Track {
            artist: artist.into(),
            title: title.into(),
            album: Some("Mezzanine".into()),
            length_ms: Some(300_000),
        }
    }

    #[test]
    fn fixes_one_song_or_every_song_by_an_artist() {
        let mut fixes = Vec::new();
        add(
            &mut fixes,
            make(
                "Unknown Artist",
                "Track 01",
                "Massive Attack",
                "Angel",
                false,
            )
            .unwrap(),
        );
        add(
            &mut fixes,
            make("RadioheadVEVO", "", "Radiohead", "", true).unwrap(),
        );
        let fixed = apply(&fixes, track("unknown artist ", "TRACK 01"));
        assert_eq!(
            (fixed.artist.as_str(), fixed.title.as_str()),
            ("Massive Attack", "Angel")
        );
        assert_eq!(fixed.album.as_deref(), Some("Mezzanine"));
        assert_eq!(
            apply(&fixes, track("Unknown Artist", "Track 02")).title,
            "Track 02"
        );
        let artist = apply(&fixes, track("RadioheadVEVO", "Creep"));
        assert_eq!(
            (artist.artist.as_str(), artist.title.as_str()),
            ("Radiohead", "Creep")
        );
        assert_eq!(apply(&fixes, track("Air", "Kelly")), track("Air", "Kelly"));
    }

    #[test]
    fn a_song_fix_wins_over_an_artist_fix() {
        let mut fixes = Vec::new();
        add(&mut fixes, make("DJ", "", "Someone", "", true).unwrap());
        add(
            &mut fixes,
            make("DJ", "Mix 3", "Other Artist", "Real Song", false).unwrap(),
        );
        assert_eq!(apply(&fixes, track("DJ", "Mix 3")).artist, "Other Artist");
        assert_eq!(apply(&fixes, track("DJ", "Mix 4")).artist, "Someone");
    }

    #[test]
    fn a_new_fix_replaces_the_old_one_for_the_same_name() {
        let mut fixes = Vec::new();
        add(&mut fixes, make("A", "x", "B", "y", false).unwrap());
        add(&mut fixes, make("a", " X ", "C", "z", false).unwrap());
        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0].new_artist, "C");
    }

    #[test]
    fn refuses_empty_or_pointless_fixes() {
        assert!(make("", "x", "B", "y", false).is_err());
        assert!(make("A", "", "B", "y", false).is_err());
        assert!(make("A", "", "", "", true).is_err());
        assert!(make("A", "x", "A", "x", false).is_err());
        assert!(make("A", "", "A", "", true).is_err());
        assert!(make("A", "x", "A", "X", false).is_ok());
        assert!(make("radiohead", "", "Radiohead", "", true).is_ok());
    }

    #[test]
    fn describes_fixes_for_the_settings_list() {
        let song = make("Unknown", "Track 01", "Air", "Alone in Kyoto", false).unwrap();
        assert_eq!(
            describe(&song),
            ("Track 01 · Unknown".into(), "Alone in Kyoto · Air".into())
        );
        let artist = make("AirVEVO", "", "Air", "", true).unwrap();
        assert_eq!(
            describe(&artist),
            ("Every song by AirVEVO".into(), "by Air".into())
        );
    }
}
