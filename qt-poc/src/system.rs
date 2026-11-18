use std::fs;
use std::path::PathBuf;

pub const FRAME_COUNTER: &str = r#"(() => {
  if (window.top !== window || window.soundscapeFrameCounter) return;
  window.soundscapeFrameCounter = true;
  const box = document.createElement("div");
  box.setAttribute("aria-hidden", "true");
  box.style.cssText = "position:fixed;top:6px;right:6px;z-index:2147483647;padding:2px 6px;border-radius:4px;background:rgba(0,0,0,.75);color:#00ff66;font:12px monospace;pointer-events:none";
  box.textContent = "… fps";
  document.documentElement.append(box);
  let frames = 0;
  let since = performance.now();
  const tick = (now) => {
    frames += 1;
    if (now - since >= 500) {
      box.textContent = `${Math.round((frames * 1000) / (now - since))} fps`;
      frames = 0;
      since = now;
    }
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
})();"#;

pub fn wants_frame_counter(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1" | "true" | "yes"))
}

pub fn normalize_server(input: &str) -> Option<String> {
    let text = input.trim().trim_end_matches('/');
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return None;
    }
    let full = if text.contains("://") {
        text.to_owned()
    } else {
        format!("https://{text}")
    };
    let (scheme, rest) = full.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || host.contains('@') || rest.contains(['?', '#']) {
        return None;
    }
    Some(format!("{scheme}://{rest}"))
}

pub fn is_own_page(server: &str, address: &str) -> bool {
    let origin = |text: &str| {
        let (scheme, rest) = text.split_once("://")?;
        let host = rest.split(['/', '?', '#']).next()?.to_ascii_lowercase();
        Some(format!("{}://{host}", scheme.to_ascii_lowercase()))
    };
    match (origin(server), origin(address)) {
        (Some(home), Some(there)) => home == there,
        _ => false,
    }
}

pub fn rss_kb(status: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|number| number.parse().ok())
}

pub fn parent_of(stat: &str) -> Option<u32> {
    let after = &stat[stat.rfind(')')? + 1..];
    after.split_whitespace().nth(1)?.parse().ok()
}

fn family(root: u32) -> Vec<u32> {
    let mut parents: Vec<(u32, u32)> = Vec::new();
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            if let Some(parent) = fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .as_deref()
                .and_then(parent_of)
            {
                parents.push((pid, parent));
            }
        }
    }
    let mut found = vec![root];
    let mut at = 0;
    while at < found.len() {
        let current = found[at];
        found.extend(
            parents
                .iter()
                .filter(|(_, parent)| *parent == current)
                .map(|(pid, _)| *pid),
        );
        at += 1;
    }
    found
}

pub fn memory_mb() -> u64 {
    let total: u64 = family(std::process::id())
        .into_iter()
        .filter_map(|pid| fs::read_to_string(format!("/proc/{pid}/status")).ok())
        .filter_map(|status| rss_kb(&status))
        .sum();
    total / 1024
}

fn settings_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("soundscape-qt").join("server"))
}

pub fn saved_server() -> Option<String> {
    let text = fs::read_to_string(settings_file()?).ok()?;
    normalize_server(&text)
}

pub fn save_server(server: &str) -> bool {
    let Some(file) = settings_file() else {
        return false;
    };
    if let Some(folder) = file.parent() {
        if fs::create_dir_all(folder).is_err() {
            return false;
        }
    }
    fs::write(file, server).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_up_the_server_address() {
        assert_eq!(
            normalize_server(" music.example.com/ ").as_deref(),
            Some("https://music.example.com")
        );
        assert_eq!(
            normalize_server("HTTP://192.168.1.5:3000").as_deref(),
            Some("http://192.168.1.5:3000")
        );
        assert_eq!(
            normalize_server("https://example.com/sound").as_deref(),
            Some("https://example.com/sound")
        );
        assert_eq!(normalize_server("ftp://example.com"), None);
        assert_eq!(normalize_server("https://user@example.com"), None);
        assert_eq!(normalize_server("https://example.com/?a=1"), None);
        assert_eq!(normalize_server("my server"), None);
        assert_eq!(normalize_server(""), None);
    }

    #[test]
    fn keeps_other_sites_out_of_the_window() {
        assert!(is_own_page(
            "https://music.example.com",
            "https://music.example.com/feed"
        ));
        assert!(is_own_page(
            "https://Music.Example.com",
            "https://music.example.com"
        ));
        assert!(!is_own_page(
            "https://music.example.com",
            "https://example.org/x"
        ));
        assert!(!is_own_page(
            "https://music.example.com",
            "http://music.example.com/x"
        ));
        assert!(!is_own_page("https://music.example.com", "nonsense"));
    }

    #[test]
    fn reads_memory_from_proc() {
        assert_eq!(
            rss_kb("Name:\tsoundscape\nVmRSS:\t  123456 kB\nThreads: 4\n"),
            Some(123456)
        );
        assert_eq!(rss_kb("Name:\tkernel\n"), None);
        assert_eq!(
            parent_of("4242 (QtWebEngine (x)) S 4200 4242 0"),
            Some(4200)
        );
        assert_eq!(parent_of("garbage"), None);
        assert!(memory_mb() > 0);
    }

    #[test]
    fn counts_frames_only_when_asked() {
        assert!(wants_frame_counter(Some("1")));
        assert!(wants_frame_counter(Some(" yes ")));
        assert!(!wants_frame_counter(Some("0")));
        assert!(!wants_frame_counter(None));
        assert!(FRAME_COUNTER.contains("requestAnimationFrame"));
    }
}
