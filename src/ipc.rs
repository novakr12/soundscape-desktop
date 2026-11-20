use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};

pub const WINDOW_FLAG: &str = "--window";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Start {
    pub url: String,
    pub server: Option<String>,
    pub native: Option<String>,
    pub size: Option<(u32, u32)>,
    pub maximized: bool,
    pub smooth: bool,
    pub memory_limit: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum ToWindow {
    Start(Start),
    Eval { script: String },
    Evaluate { id: u64, script: String },
    Show,
    Hide,
    Focus,
    Navigate { url: String },
    Server { url: Option<String> },
    Setup,
    Smooth { on: bool },
    Memory { mb: u32 },
    Exit,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum FromWindow {
    Answer {
        id: u64,
        text: Option<String>,
    },
    Url {
        url: String,
    },
    Hidden {
        hidden: bool,
    },
    Page {
        message: String,
    },
    Follow {
        url: String,
    },
    Close,
    Shortcut {
        name: String,
    },
    Poke,
    WindowState {
        width: u32,
        height: u32,
        maximized: bool,
    },
    CpuRendering {
        on: bool,
    },
    Done,
}

pub fn send<T: Serialize>(out: &mut impl Write, message: &T) -> bool {
    let Ok(mut line) = serde_json::to_string(message) else {
        return false;
    };
    line.push('\n');
    out.write_all(line.as_bytes())
        .and_then(|_| out.flush())
        .is_ok()
}

pub fn read<T: for<'a> Deserialize<'a>>(input: &mut impl BufRead) -> Option<Option<T>> {
    let mut line = String::new();
    match input.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(serde_json::from_str(line.trim_end()).ok()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_one_message_per_line_both_ways() {
        let mut wire = Vec::new();
        assert!(send(
            &mut wire,
            &ToWindow::Evaluate {
                id: 7,
                script: "1 + 1\nnext".into()
            }
        ));
        assert!(send(&mut wire, &ToWindow::Show));
        assert!(send(
            &mut wire,
            &FromWindow::Answer {
                id: 7,
                text: Some("2".into())
            }
        ));
        assert_eq!(wire.iter().filter(|byte| **byte == b'\n').count(), 3);
        let mut input = std::io::Cursor::new(wire);
        assert_eq!(
            read::<ToWindow>(&mut input),
            Some(Some(ToWindow::Evaluate {
                id: 7,
                script: "1 + 1\nnext".into()
            }))
        );
        assert_eq!(read::<ToWindow>(&mut input), Some(Some(ToWindow::Show)));
        assert_eq!(
            read::<FromWindow>(&mut input),
            Some(Some(FromWindow::Answer {
                id: 7,
                text: Some("2".into())
            }))
        );
        assert_eq!(read::<FromWindow>(&mut input), None);
    }

    #[test]
    fn skips_lines_it_cannot_read() {
        let mut input = std::io::Cursor::new(b"not json\n{\"do\":\"poke\"}\n".to_vec());
        assert_eq!(read::<FromWindow>(&mut input), Some(None));
        assert_eq!(read::<FromWindow>(&mut input), Some(Some(FromWindow::Poke)));
    }

    #[test]
    fn carries_the_start_of_a_window() {
        let start = Start {
            url: "https://music.example.com/".into(),
            server: Some("https://music.example.com/".into()),
            native: None,
            size: Some((900, 700)),
            maximized: false,
            smooth: true,
            memory_limit: 500,
        };
        let mut wire = Vec::new();
        send(&mut wire, &ToWindow::Start(start.clone()));
        let mut input = std::io::Cursor::new(wire);
        assert_eq!(
            read::<ToWindow>(&mut input),
            Some(Some(ToWindow::Start(start)))
        );
    }
}
