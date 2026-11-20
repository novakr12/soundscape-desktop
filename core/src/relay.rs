use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;
use std::time::Duration;
use url::Url;

const LONGEST_HEAD: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct Relay {
    port: u16,
    secret: String,
}

fn new_secret() -> Result<String, String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|error| error.to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[derive(Debug, PartialEq, Eq)]
pub struct Asked {
    pub kind: String,
    pub id: String,
    pub range: Option<String>,
}

pub fn parse_head(head: &str, secret: &str) -> Option<Asked> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    if first.next()? != "GET" {
        return None;
    }
    let path = first.next()?;
    let mut parts = path.trim_start_matches('/').splitn(4, '/');
    if parts.next()? != "s" || parts.next()? != secret {
        return None;
    }
    let kind = parts.next()?;
    if kind != "full-song" && kind != "listen" {
        return None;
    }
    let id = parts
        .next()
        .filter(|id| !id.is_empty() && !id.contains('/'))?;
    let range = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("range")
            .then(|| value.trim().to_string())
    });
    Some(Asked {
        kind: kind.to_string(),
        id: id.to_string(),
        range,
    })
}

pub fn content_range(value: &str) -> Option<(u64, u64, u64)> {
    let rest = value.trim().strip_prefix("bytes ")?;
    let (span, total) = rest.split_once('/')?;
    let (start, end) = span.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?, total.parse().ok()?))
}

pub fn response_head(status: u16, kind: &str, range: Option<&str>, length: Option<u64>) -> String {
    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        416 => "Range Not Satisfiable",
        404 => "Not Found",
        _ => "Bad Gateway",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n");
    let length = range
        .and_then(content_range)
        .map(|(start, end, _)| end + 1 - start)
        .or(length);
    if let Some(range) = range {
        head.push_str(&format!("Content-Range: {range}\r\n"));
    }
    if let Some(length) = length {
        head.push_str(&format!("Content-Length: {length}\r\n"));
    }
    head.push_str("\r\n");
    head
}

fn client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .read_timeout(Duration::from_secs(30))
                .user_agent(concat!("SoundscapeDesktop/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("HTTP client")
        })
        .clone()
}

fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || head.len() > LONGEST_HEAD {
            return None;
        }
        head.push_str(&line);
        if line == "\r\n" || line == "\n" {
            return Some(head.replace("\r\n", "\n").replace('\n', "\r\n"));
        }
    }
}

fn fail(stream: &mut TcpStream, status: u16) {
    let _ = stream.write_all(response_head(status, "text/plain", None, Some(0)).as_bytes());
}

fn serve(mut stream: TcpStream, server: Url, token: String, secret: String) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Some(asked) = read_head(&mut stream)
        .as_deref()
        .and_then(|head| parse_head(head, &secret))
    else {
        return fail(&mut stream, 404);
    };
    let Ok(mut target) = server.join("/") else {
        return fail(&mut stream, 502);
    };
    if let Ok(mut segments) = target.path_segments_mut() {
        segments
            .clear()
            .push("api")
            .push(&asked.kind)
            .push(&asked.id);
    }
    let whole = asked.range.is_none();
    let range = asked.range.unwrap_or_else(|| "bytes=0-".to_string());
    crate::host::async_runtime::block_on(async move {
        let upstream = client()
            .get(target)
            .header("authorization", format!("Token {token}"))
            .header("range", range)
            .send()
            .await;
        let Ok(mut upstream) = upstream else {
            return fail(&mut stream, 502);
        };
        let status = upstream.status().as_u16();
        if status != 200 && status != 206 {
            return fail(&mut stream, if status == 416 { 416 } else { 404 });
        }
        let header = |name: &str| {
            upstream
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let kind = header("content-type").unwrap_or_else(|| "application/octet-stream".into());
        let range = header("content-range").filter(|_| status == 206);
        let head = match range.as_deref().and_then(content_range) {
            Some((0, _, total)) if whole => response_head(200, &kind, None, Some(total)),
            _ => response_head(status, &kind, range.as_deref(), upstream.content_length()),
        };
        if stream.write_all(head.as_bytes()).is_err() {
            return;
        }
        while let Ok(Some(chunk)) = upstream.chunk().await {
            if stream.write_all(&chunk).is_err() {
                return;
            }
        }
    });
}

impl Relay {
    pub fn start(server: Url, token: String) -> Result<Relay, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| error.to_string())?;
        let secret = new_secret()?;
        let expected = secret.clone();
        let port = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        std::thread::Builder::new()
            .name("soundscape-relay".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let server = server.clone();
                    let token = token.clone();
                    let secret = expected.clone();
                    let _ = std::thread::Builder::new()
                        .name("soundscape-stream".into())
                        .spawn(move || serve(stream, server, token, secret));
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Relay { port, secret })
    }

    pub fn address(&self, kind: &str, id: &str) -> Option<Url> {
        let mut url = Url::parse(&format!("http://127.0.0.1:{}/", self.port)).ok()?;
        url.path_segments_mut()
            .ok()?
            .clear()
            .push("s")
            .push(&self.secret)
            .push(kind)
            .push(id);
        Some(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_player_asks_for() {
        let head =
            "GET /s/k3y/full-song/nd-1 HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: bytes=100-\r\n\r\n";
        assert_eq!(
            parse_head(head, "k3y"),
            Some(Asked {
                kind: "full-song".into(),
                id: "nd-1".into(),
                range: Some("bytes=100-".into()),
            })
        );
        assert_eq!(
            parse_head("GET /s/k3y/listen/band-song HTTP/1.1\r\n\r\n", "k3y")
                .map(|asked| asked.range),
            Some(None)
        );
        assert_eq!(
            parse_head("GET /s/full-song/nd-1 HTTP/1.1\r\n\r\n", "k3y"),
            None
        );
        assert_eq!(
            parse_head("GET /s/wrong/full-song/nd-1 HTTP/1.1\r\n\r\n", "k3y"),
            None
        );
        assert_eq!(
            parse_head("GET /s/k3y/admin/x HTTP/1.1\r\n\r\n", "k3y"),
            None
        );
        assert_eq!(
            parse_head("POST /s/k3y/full-song/x HTTP/1.1\r\n\r\n", "k3y"),
            None
        );
        assert_eq!(
            parse_head("GET /s/k3y/full-song/a/b HTTP/1.1\r\n\r\n", "k3y"),
            None
        );
        assert_eq!(parse_head("GET /other HTTP/1.1\r\n\r\n", "k3y"), None);
        assert_eq!(new_secret().unwrap().len(), 32);
        assert_ne!(new_secret().unwrap(), new_secret().unwrap());
    }

    #[test]
    fn tells_the_player_how_long_the_song_is() {
        assert_eq!(
            content_range("bytes 0-482732/482733"),
            Some((0, 482_732, 482_733))
        );
        assert_eq!(content_range("bytes */100"), None);
        let head = response_head(206, "audio/mpeg", Some("bytes 100-199/1000"), None);
        assert!(head.starts_with("HTTP/1.1 206 Partial Content\r\n"));
        assert!(head.contains("Content-Length: 100\r\n"));
        assert!(head.contains("Content-Range: bytes 100-199/1000\r\n"));
        assert!(head.contains("Accept-Ranges: bytes\r\n"));
        let plain = response_head(200, "audio/mpeg", None, Some(5000));
        assert!(plain.contains("Content-Length: 5000\r\n"));
        assert!(!plain.contains("Content-Range"));
        assert!(!response_head(200, "audio/mpeg", None, None).contains("Content-Length"));
    }
}
