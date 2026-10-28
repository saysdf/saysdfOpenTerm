//! External editor (design D8): the command comes as a JSON array in `--editor-cmd`,
//! `{path}`/`{line}`/`{col}` are substituted per element, the process is started without a shell.

use std::path::Path;
use std::process::Command;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-pager");

/// Parses `--editor-cmd`: a non-empty JSON array of strings.
pub fn parse(json: &str) -> Result<Vec<String>, String> {
    let v: Vec<String> =
        serde_json::from_str(json).map_err(|e| I18N.f("--editor-cmd: a JSON array of strings is expected: {error}", &[("error", &e.to_string())]))?;
    if v.is_empty() || v[0].is_empty() {
        return Err(I18N.t("--editor-cmd: empty command").into_owned());
    }
    Ok(v)
}

/// Substitution into each element separately: values are not split into arguments or interpreted.
pub fn substitute(template: &[String], path: &str, line: usize, col: usize) -> Vec<String> {
    let (line, col) = (line.to_string(), col.to_string());
    template
        .iter()
        .map(|a| a.replace("{path}", path).replace("{line}", &line).replace("{col}", &col))
        .collect()
}

/// Runs the editor and waits for it to finish (terminal editors work in this same pane;
/// GUI editors usually return at once).
pub fn run(template: &[String], path: &Path, line: usize, col: usize) -> Result<(), String> {
    let argv = substitute(template, &path.to_string_lossy(), line, col);
    let status = Command::new(&argv[0])
        .args(&argv[1..])
        .status()
        .map_err(|e| I18N.f("cannot start {program}: {error}", &[("program", &argv[0]), ("error", &e.to_string())]))?;
    if status.success() {
        Ok(())
    } else {
        Err(I18N.f("the editor exited with {status}", &[("status", &status.to_string())]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_per_element() {
        let t = parse(r#"["code", "-g", "{path}:{line}:{col}"]"#).unwrap();
        assert_eq!(
            substitute(&t, "a b; rm -rf.rs", 42, 7),
            vec!["code", "-g", "a b; rm -rf.rs:42:7"]
        );
    }

    #[test]
    fn error_text_follows_language() {
        {
            let _lang = saysdf_i18n::testing::use_lang("en");
            assert_eq!(parse("[]").unwrap_err(), "--editor-cmd: empty command");
        }
        let _lang = saysdf_i18n::testing::use_lang("ru");
        assert_eq!(parse("[]").unwrap_err(), "--editor-cmd: пустая команда"); // i18n-allow-cyrillic
    }

    #[test]
    fn rejects_bad_json() {
        assert!(parse("code -g").is_err());
        assert!(parse("[]").is_err());
        assert!(parse("[1]").is_err());
    }
}
