use crate::scrobble::Track;

const LONGEST_SONG_MS: u64 = 20 * 60 * 1000;
const SEPARATORS: &[&str] = &[" - ", " – ", " — "];
const NOISE: &[&str] = &[
    "official",
    "video",
    "audio",
    "lyric",
    "lyrics",
    "visualizer",
    "visualiser",
    "hd",
    "hq",
    "4k",
    "mv",
    "m/v",
];
const TRAILING: &[&str] = &[
    "official music video",
    "official video",
    "official audio",
    "official lyric video",
    "lyric video",
    "music video",
];

#[derive(PartialEq, Eq)]
enum Channel {
    Topic,
    Vevo,
    Other,
}

fn channel(name: &str) -> (String, Channel) {
    let name = name.trim();
    if let Some(artist) = name.strip_suffix(" - Topic") {
        return (artist.trim().to_string(), Channel::Topic);
    }
    for suffix in ["VEVO", "Vevo"] {
        if let Some(artist) = name.strip_suffix(suffix) {
            if !artist.trim().is_empty() {
                return (artist.trim().to_string(), Channel::Vevo);
            }
        }
    }
    (name.to_string(), Channel::Other)
}

fn noisy(inside: &str) -> bool {
    inside
        .to_lowercase()
        .split(|c: char| c.is_whitespace() || c == '-' || c == '|')
        .any(|word| NOISE.contains(&word))
}

fn without_noise(title: &str) -> String {
    let mut kept = String::new();
    let mut rest = title;
    while let Some(start) = rest.find(['(', '[', '【']) {
        let open = rest[start..].chars().next().unwrap_or('(');
        let close = match open {
            '(' => ')',
            '[' => ']',
            _ => '】',
        };
        let Some(length) = rest[start..].find(close) else {
            break;
        };
        let end = start + length + close.len_utf8();
        let inside = &rest[start + open.len_utf8()..start + length];
        kept.push_str(&rest[..start]);
        if !noisy(inside) {
            kept.push_str(&rest[start..end]);
        }
        rest = &rest[end..];
    }
    kept.push_str(rest);
    let mut text = kept
        .split(" | ")
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    loop {
        let lower = text.to_lowercase();
        let Some(suffix) = TRAILING.iter().find(|suffix| lower.ends_with(*suffix)) else {
            break;
        };
        text.truncate(text.len() - suffix.len());
        text = text.trim_end_matches([' ', '-', '–', '—', '|']).to_string();
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn split(title: &str) -> Option<(String, String)> {
    SEPARATORS.iter().find_map(|separator| {
        let (artist, song) = title.split_once(separator)?;
        let (artist, song) = (artist.trim(), song.trim());
        (!artist.is_empty() && !song.is_empty()).then(|| (artist.to_string(), song.to_string()))
    })
}

pub fn from_browser(track: Track) -> Option<Track> {
    if track
        .length_ms
        .is_some_and(|length| length > LONGEST_SONG_MS)
    {
        return None;
    }
    let (artist, kind) = channel(&track.artist);
    let title = without_noise(&track.title);
    let (artist, title, looks_like_a_song) = match split(&title) {
        Some((artist, song)) if kind != Channel::Topic => (artist, song, true),
        _ => (
            artist,
            title,
            kind != Channel::Other || track.album.is_some(),
        ),
    };
    if !looks_like_a_song || artist.is_empty() || title.is_empty() {
        return None;
    }
    Some(Track {
        artist,
        title,
        ..track
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(channel: &str, title: &str) -> Track {
        Track {
            artist: channel.into(),
            title: title.into(),
            album: None,
            length_ms: Some(240_000),
        }
    }

    fn tidy(channel: &str, title: &str) -> Option<(String, String)> {
        from_browser(video(channel, title)).map(|track| (track.artist, track.title))
    }

    fn song(artist: &str, title: &str) -> Option<(String, String)> {
        Some((artist.into(), title.into()))
    }

    #[test]
    fn takes_the_artist_from_the_video_title() {
        assert_eq!(
            tidy("RadioheadVEVO", "Radiohead - Creep (Official Video)"),
            song("Radiohead", "Creep")
        );
        assert_eq!(
            tidy(
                "Some Uploader",
                "Daft Punk – Instant Crush [Official Audio]"
            ),
            song("Daft Punk", "Instant Crush")
        );
        assert_eq!(
            tidy(
                "Label",
                "Tame Impala - Let It Happen | Official Music Video"
            ),
            song("Tame Impala", "Let It Happen")
        );
        assert_eq!(
            tidy("Label", "Bonobo - Kerala Official Video"),
            song("Bonobo", "Kerala")
        );
    }

    #[test]
    fn trusts_topic_and_vevo_channels() {
        assert_eq!(
            tidy("Nils Frahm - Topic", "Says"),
            song("Nils Frahm", "Says")
        );
        assert_eq!(
            tidy("Khruangbin - Topic", "Time (You and I)"),
            song("Khruangbin", "Time (You and I)")
        );
        assert_eq!(
            tidy("ArcticMonkeysVEVO", "Do I Wanna Know? (Official Video)"),
            song("ArcticMonkeys", "Do I Wanna Know?")
        );
    }

    #[test]
    fn keeps_parts_of_the_title_that_name_a_version() {
        assert_eq!(
            tidy("Label", "Air - La Femme d'Argent (Live at Montreux) [HD]"),
            song("Air", "La Femme d'Argent (Live at Montreux)")
        );
        assert_eq!(
            tidy(
                "Label",
                "Massive Attack - Teardrop (feat. Elizabeth Fraser)"
            ),
            song("Massive Attack", "Teardrop (feat. Elizabeth Fraser)")
        );
    }

    #[test]
    fn leaves_ordinary_videos_out() {
        assert_eq!(tidy("Cooking Channel", "How to make bread at home"), None);
        assert_eq!(tidy("Tech Channel", "Unboxing the new phone (4K)"), None);
        let mix = Track {
            length_ms: Some(3_600_000),
            ..video("Label", "Bonobo - Live Mix")
        };
        assert_eq!(from_browser(mix), None);
    }

    #[test]
    fn keeps_music_sites_that_send_the_album() {
        let music = Track {
            album: Some("In Rainbows".into()),
            ..video("Radiohead", "Weird Fishes/Arpeggi")
        };
        assert_eq!(from_browser(music.clone()), Some(music));
    }
}
