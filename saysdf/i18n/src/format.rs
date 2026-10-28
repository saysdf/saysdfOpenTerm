//! Placeholders (`{name}`, `{{`/`}}` escapes) and plural categories.
use std::collections::{BTreeMap, BTreeSet};

/// Longest result of [`format`], in bytes: past it the text is cut and ends with `…`, so a
/// template that repeats a large argument cannot build a huge string (security review L2).
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Names of the `{name}` placeholders in `s`. Error on an unbalanced brace or a bad name.
pub fn placeholders(s: &str) -> Result<BTreeSet<String>, String> {
    Ok(placeholder_counts(s)?.into_keys().collect())
}

/// Placeholder name -> number of its occurrences in `s`. Errors as [`placeholders`].
pub fn placeholder_counts(s: &str) -> Result<BTreeMap<String, usize>, String> {
    let mut out = BTreeMap::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
            }
            '{' => {
                let mut name = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) if c.is_ascii_alphanumeric() || c == '_' => name.push(c),
                        Some(c) => return Err(format!("bad character {c:?} in placeholder")),
                        None => return Err("unclosed '{'".into()),
                    }
                }
                if name.is_empty() {
                    return Err("empty placeholder '{}'".into());
                }
                *out.entry(name).or_insert(0) += 1;
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
            }
            '}' => return Err("unmatched '}' (write '}}')".into()),
            _ => {}
        }
    }
    Ok(out)
}

/// Substitutes `{name}` with the value from `args`; unknown placeholders stay as written,
/// `{{` and `}}` become `{` and `}`. Never fails: a malformed template is copied as is.
/// The result is at most [`MAX_OUTPUT_BYTES`] (plus the closing `…`).
pub fn format(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = format_unbounded(template, args);
    if out.len() > MAX_OUTPUT_BYTES {
        let mut end = MAX_OUTPUT_BYTES;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push('…');
    }
    out
}

fn format_unbounded(template: &str, args: &[(&str, &str)]) -> String {
    if !template.contains(['{', '}']) {
        return template.to_string();
    }
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(pos) = rest.find(['{', '}']) {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if tail.starts_with('{') {
            if let Some(end) = tail.find('}') {
                let name = &tail[1..end];
                if let Some((_, v)) = args.iter().find(|(k, _)| *k == name) {
                    if out.len() + v.len() > MAX_OUTPUT_BYTES {
                        // Over the limit: stop building and keep a prefix that is still too
                        // long, so `format` sees the overflow, cuts at the limit and adds `…`
                        let mut end = (MAX_OUTPUT_BYTES + 4).saturating_sub(out.len()).min(v.len());
                        while !v.is_char_boundary(end) {
                            end += 1;
                        }
                        out.push_str(&v[..end]);
                        return out;
                    }
                    out.push_str(v);
                    rest = &tail[end + 1..];
                    continue;
                }
                out.push_str(&tail[..=end]);
                rest = &tail[end + 1..];
                continue;
            }
        }
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// CLDR plural category (`one`, `few`, `many`, `other`) of the integer `n` in `lang`.
/// Languages without a rule here use `one` for 1 and `other` otherwise.
pub fn plural_category(lang: &str, n: u64) -> &'static str {
    let base = lang.split(['-', '_']).next().unwrap_or("");
    let (m10, m100) = (n % 10, n % 100);
    match base {
        "ru" | "uk" | "be" => {
            if m10 == 1 && m100 != 11 {
                "one"
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                "few"
            } else {
                "many"
            }
        }
        "pl" => {
            if n == 1 {
                "one"
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                "few"
            } else {
                "many"
            }
        }
        "cs" | "sk" => match n {
            1 => "one",
            2..=4 => "few",
            _ => "other",
        },
        "fr" => {
            if n <= 1 {
                "one"
            } else {
                "other"
            }
        }
        "ja" | "zh" | "ko" | "vi" | "th" | "id" => "other",
        _ => {
            if n == 1 {
                "one"
            } else {
                "other"
            }
        }
    }
}

/// Categories a complete plural entry needs in `lang`.
pub fn plural_categories(lang: &str) -> &'static [&'static str] {
    match lang.split(['-', '_']).next().unwrap_or("") {
        "ru" | "uk" | "be" | "pl" => &["one", "few", "many"],
        "cs" | "sk" => &["one", "few", "other"],
        "ja" | "zh" | "ko" | "vi" | "th" | "id" => &["other"],
        _ => &["one", "other"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_parse() {
        let p = placeholders("{n} files in {dir}, {{literal}}").unwrap();
        assert_eq!(p.into_iter().collect::<Vec<_>>(), ["dir", "n"]);
        assert!(placeholders("{oops").is_err());
        assert!(placeholders("a } b").is_err());
        assert!(placeholders("{}").is_err());
        assert!(placeholders("{a b}").is_err());
    }

    #[test]
    fn format_substitutes() {
        assert_eq!(format("plain", &[]), "plain");
        assert_eq!(format("{n} of {m}", &[("n", "1"), ("m", "2")]), "1 of 2");
        assert_eq!(format("{x} {{y}} }}", &[]), "{x} {y} }");
        assert_eq!(format("bad { brace", &[]), "bad { brace");
        assert_eq!(format("{a}{a}", &[("a", "z")]), "zz");
    }

    #[test]
    fn literal_brace_escapes() {
        // A config snippet in a message: no placeholders, braces rendered literally
        let s = "help = {{ banner = false }}";
        assert!(placeholders(s).unwrap().is_empty());
        assert_eq!(format(s, &[]), "help = { banner = false }");
        // Escapes next to a real placeholder
        assert_eq!(placeholders("{{{n}}}").unwrap().into_iter().collect::<Vec<_>>(), ["n"]);
        assert_eq!(format("{{{n}}}", &[("n", "3")]), "{3}");
        assert_eq!(format("{{n}}", &[("n", "3")]), "{n}");
        // A single brace is still an error for validation
        assert!(placeholders("help = { banner = false }").is_err());
    }

    #[test]
    fn placeholder_counts_repeats() {
        let c = placeholder_counts("{a} {b} {a} {{a}}").unwrap();
        assert_eq!(c.into_iter().collect::<Vec<_>>(), [("a".into(), 2), ("b".into(), 1)]);
    }

    #[test]
    fn format_output_is_capped() {
        // A large argument repeated: the result stops at the limit and is marked with `…`
        let big = "\u{44F}".repeat(MAX_OUTPUT_BYTES); // 2 bytes per char
        let out = format("{t}{t}{t}{t}", &[("t", &big)]);
        assert!(out.len() <= MAX_OUTPUT_BYTES + '…'.len_utf8(), "{}", out.len());
        assert!(out.ends_with('…'));
        // Under the limit nothing changes
        let small = "x".repeat(1000);
        assert_eq!(format("{t}-{t}", &[("t", &small)]), format!("{small}-{small}"));
        // A long template without placeholders is capped too
        assert!(format(&"y".repeat(MAX_OUTPUT_BYTES + 10), &[]).ends_with('…'));
    }

    #[test]
    fn russian_plurals() {
        let cat = |n| plural_category("ru", n);
        for (n, c) in [
            (0, "many"),
            (1, "one"),
            (2, "few"),
            (4, "few"),
            (5, "many"),
            (11, "many"),
            (12, "many"),
            (14, "many"),
            (21, "one"),
            (22, "few"),
            (25, "many"),
            (101, "one"),
            (111, "many"),
            (112, "many"),
            (122, "few"),
        ] {
            assert_eq!(cat(n), c, "n = {n}");
        }
        assert_eq!(plural_category("ru-RU", 3), "few");
    }

    #[test]
    fn default_plurals() {
        assert_eq!(plural_category("en", 1), "one");
        assert_eq!(plural_category("en", 0), "other");
        assert_eq!(plural_category("de", 2), "other");
        assert_eq!(plural_category("ja", 1), "other");
    }
}
