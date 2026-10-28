//! `saysdf help` (change startup-help, design D2): the built-in help `help.md` + the hotkey table
//! from `saysdf/keymap.md` (one source, embedded at build time) in the shared Markdown viewer.
//!
//! `saysdf/cli/help.md` is the English source; a translation is a whole document
//! `saysdf/locales/<lang>/help.md` listed in [`TRANSLATIONS`], picked by the current language
//! (change i18n). The hotkey table is English in every language for now.

use clap::Args;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-main");

const HELP_MD: &str = include_str!("../../help.md");
const KEYMAP_MD: &str = include_str!("../../../keymap.md");

/// Translated help documents: `(language, text)`.
const TRANSLATIONS: &[(&str, &str)] = &[("ru", include_str!("../../../locales/ru/help.md"))];

#[derive(Args)]
pub struct HelpArgs {
    /// Print the help text to stdout (no viewer)
    #[arg(long)]
    pub print: bool,
}

/// Help document of `lang`: its translation, else the one of its base language, else English.
fn document(lang: &str) -> &'static str {
    let mut cand = lang;
    loop {
        if let Some((_, text)) = TRANSLATIONS.iter().find(|(l, _)| *l == cand) {
            return text;
        }
        match cand.rfind('-') {
            Some(i) => cand = &cand[..i],
            None => return HELP_MD,
        }
    }
}

/// The hotkey table of keymap.md: table rows and the notes after it (no title or intro).
fn keymap_section() -> String {
    let mut out = format!("\n## {}\n\n", I18N.t("Hotkeys"));
    let mut started = false;
    for line in KEYMAP_MD.lines() {
        if !started && line.starts_with('|') {
            started = true;
        }
        if started {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

pub fn text() -> String {
    let mut s = document(&saysdf_i18n::language()).to_owned();
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(&keymap_section());
    s
}

pub fn run(args: HelpArgs) -> i32 {
    let t = text();
    if args.print {
        print!("{t}");
        return 0;
    }
    super::md::run_text("saysdf help", &t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_has_f1_f9_and_keymap() {
        let t = text();
        assert!(t.starts_with("# saysdfTerm — help"));
        assert!(t.contains("## Security"));
        assert!(t.contains("## Hotkeys"));
        assert!(t.contains("| F9 |"));
        assert!(t.contains("| F1 |"));
        assert!(t.contains("Ctrl+Shift+S"));
        assert!(t.contains("enable_kitty_keyboard = false"));
        assert!(t.contains("the typed US letter"));
    }

    #[test]
    fn help_follows_language() {
        let ru = {
            let _lang = saysdf_i18n::testing::use_lang("ru");
            text()
        };
        assert!(ru.contains("## Безопасность")); // i18n-allow-cyrillic
        assert!(ru.contains("## Хоткеи")); // i18n-allow-cyrillic
        assert!(ru.contains("| F9 |"));
        assert!(!ru.contains("## Security"));
        assert_eq!(document("ru-RU"), document("ru"));
        assert_eq!(document("de"), HELP_MD);
    }

    /// Every `saysdf/locales/<lang>/help.md` is embedded and has the English source's sections.
    #[test]
    fn every_translated_help_has_english_source() {
        let locales = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../locales");
        let headings = |s: &str| s.lines().filter(|l| l.starts_with('#')).count();
        let mut found = Vec::new();
        for e in std::fs::read_dir(&locales).unwrap().flatten() {
            let p = e.path().join("help.md");
            if !p.is_file() {
                continue;
            }
            let lang = e.file_name().to_string_lossy().to_string();
            let on_disk = std::fs::read_to_string(&p).unwrap().replace("\r\n", "\n");
            let embedded = TRANSLATIONS.iter().find(|(l, _)| *l == lang);
            let embedded = embedded.unwrap_or_else(|| panic!("{lang}/help.md is not in TRANSLATIONS"));
            assert_eq!(embedded.1.replace("\r\n", "\n"), on_disk, "{lang}/help.md");
            assert_eq!(
                headings(&on_disk),
                headings(HELP_MD),
                "{lang}/help.md: sections differ from saysdf/cli/help.md"
            );
            found.push(lang);
        }
        assert_eq!(found.len(), TRANSLATIONS.len(), "TRANSLATIONS lists a missing file");
    }
}
