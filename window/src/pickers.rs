use serde::Serialize;
use servo::{DeviceIntRect, SelectElementOptionOrOptgroup};
use std::path::PathBuf;
use std::process::Command;

const PICKER: &str = include_str!("picker.js");
pub const CLOSE_PICKER: &str = r#"document.getElementById("soundscape-picker")?.remove()"#;

#[derive(Debug, PartialEq, Serialize)]
pub struct Row {
    pub label: String,
    pub id: Option<usize>,
    pub group: bool,
    pub disabled: bool,
    pub chosen: bool,
}

pub fn rows(options: &[SelectElementOptionOrOptgroup], chosen: &[usize]) -> Vec<Row> {
    let mut rows = Vec::new();
    for entry in options {
        match entry {
            SelectElementOptionOrOptgroup::Option(option) => rows.push(Row {
                label: option.label.clone(),
                id: Some(option.id),
                group: false,
                disabled: option.is_disabled,
                chosen: chosen.contains(&option.id),
            }),
            SelectElementOptionOrOptgroup::Optgroup { label, options } => {
                rows.push(Row {
                    label: label.clone(),
                    id: None,
                    group: true,
                    disabled: true,
                    chosen: false,
                });
                for option in options {
                    rows.push(Row {
                        label: option.label.clone(),
                        id: Some(option.id),
                        group: false,
                        disabled: option.is_disabled,
                        chosen: chosen.contains(&option.id),
                    });
                }
            }
        }
    }
    rows
}

pub fn picker_script(nonce: &str, rows: &[Row], rect: DeviceIntRect) -> String {
    let place = serde_json::json!({
        "x": rect.min.x,
        "y": rect.min.y,
        "width": rect.width(),
        "height": rect.height(),
    });
    PICKER
        .replacen(
            "NONCE",
            &serde_json::to_string(nonce).unwrap_or_default(),
            1,
        )
        .replacen("ITEMS", &serde_json::to_string(rows).unwrap_or_default(), 1)
        .replacen("RECT", &place.to_string(), 1)
}

#[derive(Debug, PartialEq)]
pub enum Picked {
    Option(usize),
    Nothing,
}

pub fn picked(nonce: &str, message: &str) -> Option<Picked> {
    let rest = message
        .trim()
        .strip_prefix(nonce)?
        .strip_prefix(":select:")?;
    if rest == "none" {
        return Some(Picked::Nothing);
    }
    rest.parse().ok().map(Picked::Option)
}

pub enum Ask {
    Alert,
    Confirm,
    Prompt(String),
    Files { many: bool },
}

pub fn arguments(ask: &Ask, message: &str) -> Vec<String> {
    let mut arguments = vec!["--title=Soundscape".to_string()];
    match ask {
        Ask::Alert => arguments.extend([
            "--info".into(),
            "--no-markup".into(),
            format!("--text={message}"),
        ]),
        Ask::Confirm => arguments.extend([
            "--question".into(),
            "--no-markup".into(),
            format!("--text={message}"),
        ]),
        Ask::Prompt(value) => arguments.extend([
            "--entry".into(),
            "--no-markup".into(),
            format!("--text={message}"),
            format!("--entry-text={value}"),
        ]),
        Ask::Files { many } => {
            arguments.push("--file-selection".into());
            if *many {
                arguments.extend(["--multiple".into(), "--separator=\n".into()]);
            }
        }
    }
    arguments
}

pub fn kdialog_arguments(ask: &Ask, message: &str) -> Vec<String> {
    let mut arguments = vec!["--title".to_string(), "Soundscape".to_string()];
    match ask {
        Ask::Alert => arguments.extend(["--msgbox".into(), message.to_string()]),
        Ask::Confirm => arguments.extend(["--yesno".into(), message.to_string()]),
        Ask::Prompt(value) => {
            arguments.extend(["--inputbox".into(), message.to_string(), value.clone()])
        }
        Ask::Files { many } => {
            if *many {
                arguments.extend(["--multiple".into(), "--separate-output".into()]);
            }
            arguments.push("--getopenfilename".into());
        }
    }
    arguments
}

pub struct Answer {
    pub ok: bool,
    pub text: String,
}

fn run_dialog(program: &str, args: Vec<String>) -> std::io::Result<Answer> {
    let output = Command::new(program).args(args).output()?;
    Ok(Answer {
        ok: output.status.success(),
        text: String::from_utf8_lossy(&output.stdout)
            .trim_end_matches('\n')
            .to_string(),
    })
}

pub fn ask(ask: &Ask, message: &str) -> Answer {
    match run_dialog("zenity", arguments(ask, message)) {
        Ok(answer) => answer,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            run_dialog("kdialog", kdialog_arguments(ask, message)).unwrap_or(Answer {
                ok: false,
                text: String::new(),
            })
        }
        Err(_) => Answer {
            ok: false,
            text: String::new(),
        },
    }
}

pub fn paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use servo::SelectElementOption;

    fn option(id: usize, label: &str) -> SelectElementOption {
        SelectElementOption {
            id,
            label: label.into(),
            is_disabled: false,
        }
    }

    #[test]
    fn lists_options_and_groups() {
        let options = vec![
            SelectElementOptionOrOptgroup::Option(option(0, "Off")),
            SelectElementOptionOrOptgroup::Optgroup {
                label: "Fades".into(),
                options: vec![option(1, "Short"), option(2, "Long")],
            },
        ];
        let rows = rows(&options, &[2]);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].label, "Fades");
        assert!(rows[1].group && rows[1].id.is_none());
        assert!(rows[3].chosen);
        assert!(!rows[0].chosen);
    }

    #[test]
    fn writes_the_picker_into_the_page() {
        let rows = vec![Row {
            label: "Off \"now\"".into(),
            id: Some(0),
            group: false,
            disabled: false,
            chosen: true,
        }];
        let script = picker_script(
            "abc",
            &rows,
            DeviceIntRect::new((10, 20).into(), (110, 60).into()),
        );
        assert!(script.contains(r#"const nonce = "abc";"#));
        assert!(script.contains(r#""label":"Off \"now\"""#));
        for part in [r#""x":10"#, r#""y":20"#, r#""width":100"#, r#""height":40"#] {
            assert!(script.contains(part), "{part}");
        }
        assert!(!script.contains("NONCE"));
    }

    #[test]
    fn hears_back_from_the_picker() {
        assert_eq!(picked("abc", "abc:select:3"), Some(Picked::Option(3)));
        assert_eq!(picked("abc", "abc:select:none"), Some(Picked::Nothing));
        assert_eq!(picked("abc", "xyz:select:3"), None);
        assert_eq!(picked("abc", "abc:select:x"), None);
        assert_eq!(picked("abc", "hello"), None);
    }

    #[test]
    fn asks_with_zenity() {
        assert_eq!(
            arguments(&Ask::Confirm, "Revoke?"),
            vec![
                "--title=Soundscape",
                "--question",
                "--no-markup",
                "--text=Revoke?"
            ]
        );
        let prompt = arguments(&Ask::Prompt("x".into()), "Name");
        assert!(prompt.contains(&"--no-markup".to_string()));
        assert!(prompt.contains(&"--entry-text=x".to_string()));
        assert!(arguments(&Ask::Files { many: true }, "").contains(&"--multiple".to_string()));
        assert!(!arguments(&Ask::Files { many: false }, "").contains(&"--multiple".to_string()));
        assert_eq!(
            kdialog_arguments(&Ask::Prompt("x".into()), "Name"),
            vec!["--title", "Soundscape", "--inputbox", "Name", "x"]
        );
        assert_eq!(
            kdialog_arguments(&Ask::Files { many: true }, ""),
            vec![
                "--title",
                "Soundscape",
                "--multiple",
                "--separate-output",
                "--getopenfilename"
            ]
        );
        assert_eq!(
            paths("/a\n/b\n"),
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }
}
