fn quoted(family: &str) -> String {
    family
        .chars()
        .filter(|letter| !letter.is_control())
        .flat_map(|letter| match letter {
            '"' | '\\' => vec!['\\', letter],
            _ => vec![letter],
        })
        .collect()
}

fn rule_json(family: Option<&str>) -> serde_json::Value {
    family.map_or(serde_json::Value::Null, |family| {
        serde_json::Value::from(stylesheet(family))
    })
}

pub fn page_script(family: Option<&str>) -> String {
    format!(
        r#"(() => {{
  const id = "soundscape-desktop-font";
  window.soundscapeDesktopFont = (css) => {{
    const place = () => {{
      let style = document.getElementById(id);
      if (!css) {{
        if (style) style.remove();
        return;
      }}
      if (!style) {{
        style = document.createElement("style");
        style.id = id;
        (document.head || document.documentElement).appendChild(style);
      }}
      style.textContent = css;
    }};
    if (document.documentElement) place();
    else document.addEventListener("DOMContentLoaded", place, {{ once: true }});
  }};
  window.soundscapeDesktopFont({});
}})();"#,
        rule_json(family)
    )
}

pub fn restyle_script(family: Option<&str>) -> String {
    format!(
        "window.soundscapeDesktopFont && window.soundscapeDesktopFont({})",
        rule_json(family)
    )
}

pub fn stylesheet(family: &str) -> String {
    format!(
        "body {{ font-family: \"{}\", system-ui, sans-serif !important; }}",
        quoted(family)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restyles_the_open_page() {
        assert_eq!(
            restyle_script(Some("Adwaita Sans")),
            r#"window.soundscapeDesktopFont && window.soundscapeDesktopFont("body { font-family: \"Adwaita Sans\", system-ui, sans-serif !important; }")"#
        );
        assert!(restyle_script(None).ends_with("(null)"));
        assert!(page_script(Some("Inter")).contains("soundscape-desktop-font"));
        assert!(page_script(None).ends_with("window.soundscapeDesktopFont(null);\n})();"));
    }

    #[test]
    fn keeps_the_page_font_rule_safe() {
        assert_eq!(
            stylesheet("Adwaita Sans"),
            "body { font-family: \"Adwaita Sans\", system-ui, sans-serif !important; }"
        );
        assert_eq!(
            stylesheet("Odd \"Font\"\\\n"),
            "body { font-family: \"Odd \\\"Font\\\"\\\\\", system-ui, sans-serif !important; }"
        );
    }
}
