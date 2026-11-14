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

const AUDIO_FILES: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "opus", "m4a", "aac", "wav", "wma", "ape", "wv", "aiff", "aif",
    "mka", "mp4",
];
const UNKNOWN_ARTISTS: &[&str] = &[
    "unknown artist",
    "unknown",
    "[unknown]",
    "<unknown>",
    "artist",
];
const UNKNOWN_ALBUMS: &[&str] = &["unknown album", "unknown", "[unknown]", "<unknown>"];

fn without_extension(title: &str) -> Option<&str> {
    let (stem, extension) = title.rsplit_once('.')?;
    (!stem.trim().is_empty() && AUDIO_FILES.contains(&extension.to_lowercase().as_str()))
        .then_some(stem)
}

fn without_number(title: &str, from_file: bool) -> &str {
    let digits = title.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return title;
    }
    let mut rest = &title[digits..];
    let mut disc = false;
    if let Some(after) = rest.strip_prefix(['-', '.']) {
        let disc_track = after.chars().take_while(char::is_ascii_digit).count();
        if (1..=3).contains(&disc_track) && after[disc_track..].starts_with([' ', '-', '.', '_']) {
            rest = &after[disc_track..];
            disc = true;
        }
    }
    let separator: String = rest
        .chars()
        .take_while(|c| matches!(c, ' ' | '-' | '.' | '_'))
        .collect();
    let punctuated = disc || separator.contains(['-', '.', '_']);
    let spaced_file_number = from_file && digits >= 2 && !separator.is_empty();
    let remaining = rest[separator.len()..].trim();
    if remaining.is_empty() || !(punctuated || spaced_file_number) {
        return title;
    }
    remaining
}

pub fn from_file(track: Track) -> Option<Track> {
    let original = track.title.trim();
    let (title, from_file) = match without_extension(original) {
        Some(stem) => (stem.trim(), true),
        None => (original, false),
    };
    let mut title = without_number(title, from_file).to_string();
    if from_file && title.contains('_') && !title.contains(' ') {
        title = title.replace('_', " ");
    }
    let mut artist = track.artist.trim().to_string();
    let weak = artist.is_empty() || UNKNOWN_ARTISTS.contains(&artist.to_lowercase().as_str());
    let various = artist.eq_ignore_ascii_case("various artists");
    if weak || various {
        match split(&title) {
            Some((found, song)) => {
                artist = found;
                title = without_number(&song, from_file).to_string();
            }
            None if weak => return None,
            None => {}
        }
    }
    let album = track
        .album
        .clone()
        .filter(|album| !UNKNOWN_ALBUMS.contains(&album.trim().to_lowercase().as_str()));
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if artist.is_empty() || title.is_empty() {
        return None;
    }
    Some(Track {
        artist,
        title,
        album,
        ..track
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

    fn file(artist: &str, title: &str) -> Option<(String, String)> {
        from_file(Track {
            artist: artist.into(),
            title: title.into(),
            album: None,
            length_ms: Some(240_000),
        })
        .map(|track| (track.artist, track.title))
    }

    #[test]
    fn leaves_tagged_songs_alone() {
        assert_eq!(file("Prince", "1999"), song("Prince", "1999"));
        assert_eq!(
            file("Ariana Grande", "7 rings"),
            song("Ariana Grande", "7 rings")
        );
        assert_eq!(file("Jay-Z", "99 Problems"), song("Jay-Z", "99 Problems"));
        assert_eq!(
            file("Air", "Talisman - Remastered"),
            song("Air", "Talisman - Remastered")
        );
        assert_eq!(
            file("Blink-182", "Adam's Song"),
            song("Blink-182", "Adam's Song")
        );
    }

    #[test]
    fn cleans_up_file_names() {
        assert_eq!(file("Air", "03 - Talisman.flac"), song("Air", "Talisman"));
        assert_eq!(file("Air", "03. Talisman"), song("Air", "Talisman"));
        assert_eq!(file("Air", "1-03 Talisman.mp3"), song("Air", "Talisman"));
        assert_eq!(file("Air", "03 Talisman.mp3"), song("Air", "Talisman"));
        assert_eq!(
            file("Air", "Kelly_Watch_The_Stars.ogg"),
            song("Air", "Kelly Watch The Stars")
        );
        assert_eq!(
            file("Air", "La Femme d'Argent.m4a"),
            song("Air", "La Femme d'Argent")
        );
    }

    #[test]
    fn finds_the_artist_in_the_title_when_the_tags_have_none() {
        assert_eq!(
            file("", "03 - Air - Talisman.flac"),
            song("Air", "Talisman")
        );
        assert_eq!(
            file("Unknown Artist", "Massive Attack - Teardrop"),
            song("Massive Attack", "Teardrop")
        );
        assert_eq!(
            file("Various Artists", "Bonobo - Kerala"),
            song("Bonobo", "Kerala")
        );
        assert_eq!(
            file("Various Artists", "Kerala"),
            song("Various Artists", "Kerala")
        );
        assert_eq!(file("", "Track 01.mp3"), None);
        assert_eq!(file("Unknown", "Intro"), None);
    }

    #[test]
    fn drops_unknown_albums() {
        let track = from_file(Track {
            artist: "Air".into(),
            title: "Talisman".into(),
            album: Some("Unknown Album".into()),
            length_ms: None,
        })
        .unwrap();
        assert_eq!(track.album, None);
    }
}
