//! Reuse the previous locale catalogues without a browser script or JS build.
use super::esc;
use butterpollo_stream::locale::{dictionary, lookup};
pub use butterpollo_stream::locale::{language_name, locale};
pub fn message(english: &str) -> String {
    label("", english)
}
pub fn label(name: &str, english: &str) -> String {
    let (section, name) = if let Some(name) = name.strip_prefix("app_") {
        ("apps", name)
    } else {
        (
            "config",
            name.strip_prefix("cfg_")
                .or_else(|| name.strip_prefix("client_"))
                .unwrap_or(name),
        )
    };
    let path = format!("/{section}/{}", name.replace('-', "_"));
    let path = dictionary("en").contains_key(&path).then_some(path);
    format!(
        "\u{fdd0}{}\u{fdd1}",
        serde_json::to_string(&(path, english))
            .unwrap()
            .replace('\u{fdd0}', "\\ufdd0")
            .replace('\u{fdd1}', "\\ufdd1")
            .replace('\u{fdd2}', "\\ufdd2")
            .replace('\u{fdd3}', "\\ufdd3")
    )
}
/// Protect configuration, names, commands and JSON from language substitution.
pub fn data(value: &str) -> String {
    let value = esc(value)
        .replace('\u{fdd0}', "&#64976;")
        .replace('\u{fdd1}', "&#64977;")
        .replace('\u{fdd2}', "&#64978;")
        .replace('\u{fdd3}', "&#64979;");
    format!("\u{fdd2}{value}\u{fdd3}")
}
pub fn render(body: &str, raw_locale: &str) -> String {
    let selected = locale(raw_locale);
    let dictionary = dictionary(selected);
    let english = selected == "en";
    let mut output = String::with_capacity(body.len());
    let mut remaining = body;
    let mut in_tag = false;
    let mut quote = None;
    while !remaining.is_empty() {
        if let Some(rest) = remaining.strip_prefix('\u{fdd2}')
            && let Some((value, rest)) = rest.split_once('\u{fdd3}')
        {
            output.push_str(value);
            remaining = rest;
            continue;
        }
        if let Some(rest) = remaining.strip_prefix('\u{fdd0}')
            && let Some((value, rest)) = rest.split_once('\u{fdd1}')
            && let Ok((path, fallback)) = serde_json::from_str::<(Option<String>, String)>(value)
        {
            let text = if english {
                &fallback
            } else {
                lookup(&dictionary, &fallback, path.as_deref()).unwrap_or(&fallback)
            };
            output.push_str(&esc(text));
            remaining = rest;
            continue;
        }
        let first = remaining.chars().next().unwrap();
        if in_tag || first == '<' {
            if first == '<' && !in_tag {
                in_tag = true;
            } else if matches!(first, '\'' | '"') {
                if quote == Some(first) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(first);
                }
            } else if first == '>' && quote.is_none() {
                in_tag = false;
            }
            output.push(first);
            remaining = &remaining[first.len_utf8()..];
        } else {
            let end = remaining
                .find(['<', '\u{fdd0}', '\u{fdd2}'])
                .unwrap_or(remaining.len())
                .max(first.len_utf8());
            let text = &remaining[..end];
            let trimmed = text.trim();
            if !english && let Some(translated) = lookup(&dictionary, trimmed, None) {
                let start = text.len() - text.trim_start().len();
                output.push_str(&text[..start]);
                output.push_str(&esc(translated));
                output.push_str(&text[start + trimmed.len()..]);
            } else {
                output.push_str(text);
            }
            remaining = &remaining[end..];
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dynamic_text_cannot_terminate_language_markers() {
        let attack = "Phone\u{fdd0}\u{fdd1}\u{fdd2}\u{fdd3}<meta http-equiv=refresh>";
        for protected in [label("", attack), data(attack)] {
            let rendered = render(&format!("<h1>{protected}</h1>"), "en");
            assert!(!rendered.contains("<meta"));
            assert!(rendered.contains("&lt;meta"));
        }
    }
    #[test]
    fn inherited_locales_translate_controls_and_preserve_names_values_and_json() {
        let body = format!(
            "<h1>{}</h1><label>{}<input value=\"{}\"></label><h2>{}</h2><pre>{}</pre><button>Save</button>",
            message("Settings"),
            label("cfg_locale", "Language"),
            data("Save"),
            data("Settings"),
            data("{\"Save\":\"Settings\"}")
        );
        let de = render(&body, "de-DE");
        assert!(de.contains("<h1>Konfiguration</h1>"));
        assert!(de.contains("<h2>Settings</h2>"));
        assert!(de.contains("value=\"Save\""));
        assert!(de.contains("{&quot;Save&quot;:&quot;Settings&quot;}"));
        assert!(de.contains("<button>Speichern</button>"));
        let malicious = data("\u{fdd0}[null,\"Settings\"]\u{fdd1}<script>");
        assert_eq!(
            render(&malicious, "de"),
            "&#64976;[null,&quot;Settings&quot;]&#64977;&lt;script&gt;"
        );
        assert_eq!(locale("../../evil"), "en");
        assert_eq!(locale("zh-TW"), "zh_TW");
        for (name, _) in butterpollo_stream::locale::CATALOGUES {
            assert!(!dictionary(name).is_empty());
        }
    }
}
