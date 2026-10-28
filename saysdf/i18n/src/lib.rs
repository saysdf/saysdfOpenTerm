//! saysdfTerm UI localization (change `i18n`).
//!
//! Source strings are English and serve as keys (gettext style). Translations are TOML data
//! files `saysdf/locales/<lang>/<domain>.toml` embedded in the binary, plus the user's
//! `<config dir>/locales/<lang>/<domain>.toml` on top. A domain is one Lua module or one area of
//! a Rust tool, so modules are migrated and translated independently.
//!
//! Rust code declares its domain once per file and looks strings up through it:
//!
//! ```
//! use saysdf_i18n::Domain;
//! static I18N: Domain = Domain::new("example");
//! let label = I18N.t("Open");
//! let msg = I18N.f("Copied {path}", &[("path", &"a.txt")]);
//! let files = I18N.n("{n} file", "{n} files", 3, &[]);
//! # assert_eq!((label.as_ref(), msg.as_str(), files.as_str()), ("Open", "Copied a.txt", "3 files"));
//! ```
//!
//! Lua modules use `saysdf.i18n` (backed by this crate through `saysdf-lua-bundle`), so there is one
//! implementation of catalogs, plurals and formatting.
use std::borrow::Cow;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

pub mod catalog;
pub mod detect;
pub mod format;
pub mod text;

pub use catalog::{Catalog, Msg};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded.rs"));
}

/// Language of the source strings.
pub const SOURCE_LANG: &str = "en";
/// `language` value that selects the system UI language.
pub const AUTO: &str = "auto";
/// Env var with the resolved language, set by the terminal for the tools it starts.
pub const LANG_ENV: &str = "SAYSDF_LANG";
/// Subdirectory of the user config dir with user catalogs.
pub const USER_LOCALES_DIR: &str = "locales";

/// Embedded catalogs: `(language, domain, TOML source)`, sorted.
pub fn embedded() -> &'static [(&'static str, &'static str, &'static str)] {
    embedded::EMBEDDED
}

/// User catalogs dir for tools started outside of the Lua config: `<user config dir>/locales`.
pub fn default_user_dir() -> Option<PathBuf> {
    saysdf_identity::user_config_dir().map(|d| d.join(USER_LOCALES_DIR))
}

/// Languages with a catalog: English, embedded and user ones; sorted, without duplicates.
pub fn languages(user_dir: Option<&Path>) -> Vec<String> {
    let mut v = vec![SOURCE_LANG.to_string()];
    v.extend(catalog::embedded_languages(embedded()));
    v.extend(catalog::user_languages(user_dir));
    v.sort();
    v.dedup();
    v
}

/// Chooses the language: `requested` is `auto`, empty or a tag (`ru`, `pt-BR`). `auto` takes
/// `env_lang` (the terminal's choice passed to a tool), else the system language. A tag
/// without a catalog falls back to its base language (`pt-BR` -> `pt`), then to English;
/// an explicitly requested language without a catalog also gives a warning.
pub fn resolve(
    requested: &str,
    env_lang: Option<&str>,
    system: Option<&str>,
    available: &[String],
) -> (String, Option<String>) {
    let requested = requested.trim();
    let auto = requested.is_empty() || requested.eq_ignore_ascii_case(AUTO);
    let wanted = if auto {
        env_lang
            .filter(|l| !l.trim().is_empty() && !l.trim().eq_ignore_ascii_case(AUTO))
            .and_then(detect::normalize)
            .or_else(|| system.and_then(detect::normalize))
    } else {
        detect::normalize(requested)
    };
    if let Some(tag) = &wanted {
        let mut cand = tag.as_str();
        loop {
            if available.iter().any(|a| a == cand) {
                return (cand.to_string(), None);
            }
            match cand.rfind('-') {
                Some(i) => cand = &cand[..i],
                None => break,
            }
        }
    }
    let warning = (!auto).then(|| {
        format!("language {requested:?} has no translation catalog; using English")
    });
    (SOURCE_LANG.to_string(), warning)
}

/// Result of [`init`].
#[derive(Debug, Clone)]
pub struct Init {
    pub language: String,
    pub warnings: Vec<String>,
}

static CURRENT: RwLock<Option<Arc<Catalog>>> = RwLock::new(None);

/// Chooses the language and loads its catalogs (embedded + `user_dir/<lang>/*.toml`) without
/// making it current. Never fails: problems come back as warnings.
pub fn load(requested: &str, user_dir: Option<&Path>) -> (Arc<Catalog>, Init) {
    let env_lang = std::env::var(LANG_ENV).ok();
    let system = detect::system_language();
    let (lang, warning) = resolve(
        requested,
        env_lang.as_deref(),
        system.as_deref(),
        &languages(user_dir),
    );
    let (cat, mut warnings) = catalog::load(&lang, embedded(), user_dir);
    warnings.splice(0..0, warning);
    (
        Arc::new(cat),
        Init {
            language: lang,
            warnings,
        },
    )
}

/// Makes `cat` the process catalog used by [`Domain`] and the free lookup functions.
/// In the GUI only `saysdf.setup` does this (through `saysdf._i18n.apply_global`); a Lua
/// `saysdf.i18n.select` changes only its own Lua state.
pub fn set_current(cat: Arc<Catalog>) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(cat);
}

/// [`load`] + [`set_current`]. `saysdf.setup` does this on every config (re)load, so Rust code
/// in the GUI process follows the configured language; tools call [`init_from_env`] at the start
/// of `main` (user catalogs are read there, never lazily by a lookup).
pub fn init(requested: &str, user_dir: Option<&Path>) -> Init {
    let (cat, init) = load(requested, user_dir);
    set_current(cat);
    init
}

/// [`init`] with `$SAYSDF_LANG` (or `auto`) and [`default_user_dir`].
pub fn init_from_env() -> Init {
    let requested = std::env::var(LANG_ENV).unwrap_or_default();
    init(&requested, default_user_dir().as_deref())
}

/// The process catalog. Without an [`init`] it is the embedded catalog of `$SAYSDF_LANG`/the
/// system language: a lookup never reads the disk, whatever thread it runs on (security
/// review L3); user catalogs come only from an explicit [`init`]/[`init_from_env`].
///
/// Test builds (feature `testing`) never use the process catalog for lookups: a thread gets
/// English, or the language of its [`testing::use_lang`] guard, whatever the system locale and
/// whatever another test passed to [`init`].
pub fn current() -> Arc<Catalog> {
    #[cfg(any(test, feature = "testing"))]
    return testing::thread_catalog();
    #[cfg(not(any(test, feature = "testing")))]
    process_catalog()
}

fn process_catalog() -> Arc<Catalog> {
    if let Some(c) = CURRENT.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return Arc::clone(c);
    }
    let requested = std::env::var(LANG_ENV).unwrap_or_default();
    let (cat, _) = load(&requested, None);
    let mut slot = CURRENT.write().unwrap_or_else(|e| e.into_inner());
    // Another thread may have initialised it meanwhile: keep that one
    Arc::clone(slot.get_or_insert(cat))
}

/// Current language tag.
pub fn language() -> String {
    current().lang.clone()
}

/// Translation of `key` in `domain` with `{name}` placeholders substituted.
pub fn lookup(domain: &str, key: &str, args: &[(&str, &str)]) -> String {
    current().text(domain, key, args)
}

/// Plural form for `count`, see [`Catalog::plural`].
pub fn lookup_plural(domain: &str, one: &str, other: &str, count: u64, args: &[(&str, &str)]) -> String {
    current().plural(domain, one, other, count, args)
}

/// Menu accelerator of the registry entry `id` in the current language, if a catalog sets one.
pub fn accel(id: &str) -> Option<String> {
    current().accel.get(id).cloned()
}

impl Catalog {
    /// Translation of `key` in `domain` with `{name}` placeholders substituted.
    pub fn text(&self, domain: &str, key: &str, args: &[(&str, &str)]) -> String {
        let template = match self.get(domain, key) {
            Some(Msg::Text(s)) => s.as_str(),
            Some(Msg::Plural(forms)) => forms.get("other").map_or(key, String::as_str),
            None => key,
        };
        format::format(template, args)
    }

    /// Plural form for `count`: the catalog's form for the language's category, else the
    /// English `one` (count 1) or `other`. `{n}` is `count` unless `args` has its own `n`.
    pub fn plural(&self, domain: &str, one: &str, other: &str, count: u64, args: &[(&str, &str)]) -> String {
        let english = if count == 1 { one } else { other };
        let template = match self.get(domain, one) {
            Some(Msg::Plural(forms)) => {
                let category = format::plural_category(&self.lang, count);
                forms
                    .get(category)
                    .or_else(|| forms.get("other"))
                    .map_or(english, String::as_str)
            }
            Some(Msg::Text(s)) => s.as_str(),
            None => english,
        };
        let n = count.to_string();
        let mut all: Vec<(&str, &str)> = args.to_vec();
        if !args.iter().any(|(k, _)| *k == "n") {
            all.push(("n", &n));
        }
        format::format(template, &all)
    }
}

/// A translation domain of Rust code: one per file, `static I18N: Domain = Domain::new("cli-db");`.
/// Keys must be string literals at the call site (`I18N.t("...")`, `I18N.f("...", ..)`,
/// `I18N.n("...", "...", ..)`): the catalog completeness test reads them from the sources.
#[derive(Clone, Copy, Debug)]
pub struct Domain(&'static str);

impl Domain {
    pub const fn new(name: &'static str) -> Self {
        Domain(name)
    }

    pub fn name(&self) -> &'static str {
        self.0
    }

    /// Translation of a string without placeholders; no allocation when untranslated.
    pub fn t(&self, key: &'static str) -> Cow<'static, str> {
        let cat = current();
        match cat.get(self.0, key) {
            Some(Msg::Text(s)) => Cow::Owned(format::format(s, &[])),
            _ if key.contains(['{', '}']) => Cow::Owned(format::format(key, &[])),
            _ => Cow::Borrowed(key),
        }
    }

    /// Translation with `{name}` placeholders.
    pub fn f(&self, key: &'static str, args: &[(&str, &dyn Display)]) -> String {
        let owned = stringify(args);
        lookup(self.0, key, &borrow(&owned))
    }

    /// Plural translation: `one`/`other` are the English forms, `{n}` is `count`.
    pub fn n(
        &self,
        one: &'static str,
        other: &'static str,
        count: u64,
        args: &[(&str, &dyn Display)],
    ) -> String {
        let owned = stringify(args);
        lookup_plural(self.0, one, other, count, &borrow(&owned))
    }
}

fn stringify<'a>(args: &[(&'a str, &dyn Display)]) -> Vec<(&'a str, String)> {
    args.iter().map(|(k, v)| (*k, v.to_string())).collect()
}

fn borrow<'a>(owned: &'a [(&'a str, String)]) -> Vec<(&'a str, &'a str)> {
    owned.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

/// Language of Rust tests (feature `testing`, enabled by dependents in `[dev-dependencies]`).
///
/// The process catalog is shared by the parallel test threads of a binary and defaults to the
/// system language, so test builds ignore it: lookups are English unless the test takes a guard
/// that sets the catalog for the calling thread only (cargo runs every test on its own thread).
/// A test never calls [`init`]/[`set_current`] to pick a language:
///
/// ```ignore
/// use saysdf_i18n::{testing, Domain};
/// static I18N: Domain = Domain::new("menu");
/// let _lang = testing::use_lang("en");
/// assert_eq!(I18N.t("View"), "View");
/// ```
///
/// Text built on another thread (spawned by the code under test) is English.
/// Without the feature the module does not exist and [`current`] has no extra check.
#[cfg(any(test, feature = "testing"))]
pub mod testing {
    use super::{catalog, embedded, Arc, Catalog};
    use std::cell::RefCell;
    use std::marker::PhantomData;
    use std::sync::OnceLock;

    thread_local! {
        static OVERRIDE: RefCell<Option<Arc<Catalog>>> = const { RefCell::new(None) };
    }

    /// Restores the previous override of its thread when dropped; not `Send`.
    #[must_use = "the language is overridden only while the guard lives"]
    pub struct LangGuard {
        prev: Option<Arc<Catalog>>,
        _not_send: PhantomData<*const ()>,
    }

    impl Drop for LangGuard {
        fn drop(&mut self) {
            let prev = self.prev.take();
            OVERRIDE.with(|o| *o.borrow_mut() = prev);
        }
    }

    /// Embedded catalog of `lang` (`"en"`, `"ru"`) for lookups on this thread while the guard
    /// lives. User catalogs, `$SAYSDF_LANG` and the system language are not consulted.
    pub fn use_lang(lang: &str) -> LangGuard {
        let (cat, warnings) = catalog::load(lang, embedded(), None);
        assert!(warnings.is_empty(), "{warnings:?}");
        use_catalog(Arc::new(cat))
    }

    /// `cat` (e.g. from [`super::load`] with a temporary user dir) for this thread.
    pub fn use_catalog(cat: Arc<Catalog>) -> LangGuard {
        let prev = OVERRIDE.with(|o| o.borrow_mut().replace(cat));
        LangGuard {
            prev,
            _not_send: PhantomData,
        }
    }

    /// Catalog of lookups on this thread: its override, else English.
    pub(crate) fn thread_catalog() -> Arc<Catalog> {
        static ENGLISH: OnceLock<Arc<Catalog>> = OnceLock::new();
        OVERRIDE.with(|o| o.borrow().clone()).unwrap_or_else(|| {
            Arc::clone(ENGLISH.get_or_init(|| {
                Arc::new(catalog::load(super::SOURCE_LANG, embedded(), None).0)
            }))
        })
    }

    /// Language of the process catalog (what [`super::init`] set), which test builds do not
    /// use for lookups.
    pub fn process_language() -> String {
        super::process_catalog().lang.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn avail(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn resolve_language() {
        let a = avail(&["en", "pt", "ru"]);
        assert_eq!(resolve("auto", None, Some("ru-RU"), &a), ("ru".into(), None));
        assert_eq!(resolve("", None, Some("de-DE"), &a), ("en".into(), None));
        assert_eq!(resolve("auto", None, None, &a), ("en".into(), None));
        assert_eq!(resolve("auto", Some("ru"), Some("en-US"), &a), ("ru".into(), None));
        assert_eq!(resolve("auto", Some("auto"), Some("ru"), &a), ("ru".into(), None));
        assert_eq!(resolve("en", None, Some("ru"), &a), ("en".into(), None));
        assert_eq!(resolve("pt-BR", None, None, &a), ("pt".into(), None));
        assert_eq!(resolve("RU", None, None, &a), ("ru".into(), None));
        let (l, w) = resolve("de", None, Some("ru"), &a);
        assert_eq!(l, "en");
        assert!(w.unwrap().contains("\"de\""));
    }

    // Lookups through a per-thread catalog (`testing::use_catalog`): no test changes the
    // language of another one.
    #[test]
    fn lookups() {
        let dir = tempfile::tempdir().unwrap();
        let xx = dir.path().join("xx");
        std::fs::create_dir(&xx).unwrap();
        std::fs::write(
            xx.join("demo.toml"),
            r#"
            "Open" = "Otvori"
            "Copied {path}" = "Kopirano {path}"
            "{n} file" = { one = "{n} fajl", other = "{n} fajlova" }
            [_accel]
            "demo.open" = "o"
            "#,
        )
        .unwrap();
        static D: Domain = Domain::new("demo");

        let (cat, r) = load("xx", Some(dir.path()));
        assert_eq!(r.language, "xx");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let _xx = testing::use_catalog(cat);
        assert_eq!(language(), "xx");
        assert_eq!(D.t("Open"), "Otvori");
        assert!(matches!(D.t("Not translated"), Cow::Borrowed("Not translated")));
        assert_eq!(D.f("Copied {path}", &[("path", &"a.txt")]), "Kopirano a.txt");
        assert_eq!(D.n("{n} file", "{n} files", 1, &[]), "1 fajl");
        assert_eq!(D.n("{n} file", "{n} files", 7, &[]), "7 fajlova");
        assert_eq!(D.n("{n} dir", "{n} dirs", 7, &[]), "7 dirs");
        assert_eq!(accel("demo.open").as_deref(), Some("o"));
        assert_eq!(Domain::new("other").t("Open"), "Open");

        let (cat, r) = load("de", Some(dir.path()));
        assert_eq!(r.language, "en");
        assert_eq!(r.warnings.len(), 1);
        {
            let _en = testing::use_catalog(cat);
            assert_eq!(D.t("Open"), "Open");
            assert_eq!(D.n("{n} file", "{n} files", 1, &[]), "1 file");
            assert_eq!(D.f("{{literal}} {path}", &[("path", &1)]), "{literal} 1");
            assert_eq!(accel("demo.open"), None);
        }
        // the inner guard restored the outer override
        assert_eq!(D.t("Open"), "Otvori");
    }

    // The only test that changes the process catalog (`init` is what `saysdf.setup` and the
    // tools call). Test builds keep looking up in English: the system locale and `init` of
    // another test never change the text a test sees.
    #[test]
    fn init_sets_process_catalog_but_tests_stay_english() {
        static M: Domain = Domain::new("menu");
        let r = init("ru", None);
        assert_eq!(r.language, "ru");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(testing::process_language(), "ru");
        assert_eq!(language(), "en");
        assert_eq!(M.t("View"), "View");
        {
            let _ru = testing::use_lang("ru");
            assert_eq!(M.t("View"), "Вид"); // i18n-allow-cyrillic
        }
        assert_eq!(M.t("View"), "View");
    }

    // Threads with different overrides run in parallel and each sees only its own language.
    #[test]
    fn thread_overrides_are_isolated() {
        static M: Domain = Domain::new("menu");
        let run = |lang: &'static str, want: &'static str| {
            std::thread::spawn(move || {
                let _g = testing::use_lang(lang);
                for _ in 0..2000 {
                    assert_eq!(M.t("View"), want);
                    assert_eq!(language(), lang);
                }
            })
        };
        let en = run("en", "View");
        let ru = run("ru", "Вид"); // i18n-allow-cyrillic
        en.join().unwrap();
        ru.join().unwrap();
    }

    #[test]
    fn embedded_catalogs_parse() {
        assert!(!embedded().is_empty());
        for (lang, domain, src) in embedded() {
            assert!(catalog::valid_lang(lang), "{lang}");
            assert!(catalog::valid_domain(domain), "{domain}");
            catalog::parse(src).unwrap_or_else(|e| panic!("{lang}/{domain}.toml: {e}"));
        }
        assert!(languages(None).contains(&"ru".to_string()));
    }
}
