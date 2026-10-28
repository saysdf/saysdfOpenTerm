//! System UI language (change `i18n`, design D5).

/// Normalizes a locale name to a language tag: `ru_RU.UTF-8@euro` -> `ru-RU`, `en-us` -> `en-US`.
/// `C`, `POSIX` and empty names have no language (`None`).
pub fn normalize(name: &str) -> Option<String> {
    let name = name.trim();
    let name = name.split(['.', '@']).next().unwrap_or("");
    if name.is_empty() || name == "C" || name == "POSIX" {
        return None;
    }
    let mut parts = name.split(['_', '-']);
    let lang = parts.next()?.to_ascii_lowercase();
    if !(2..=3).contains(&lang.len()) || !lang.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    let mut out = lang;
    for p in parts {
        if !(2..=8).contains(&p.len()) || !p.chars().all(|c| c.is_ascii_alphanumeric()) {
            break;
        }
        out.push('-');
        if p.len() == 2 {
            out.push_str(&p.to_ascii_uppercase());
        } else if p.len() == 4 {
            // Script subtag: Hans, Latn
            out.push_str(&p[..1].to_ascii_uppercase());
            out.push_str(&p[1..].to_ascii_lowercase());
        } else {
            out.push_str(p);
        }
    }
    Some(out)
}

/// First non-empty POSIX locale variable in priority order: `LC_ALL`, `LC_MESSAGES`, `LANG`.
pub fn from_posix_env(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| get(v))
        .find(|v| !v.trim().is_empty())
        .and_then(|v| normalize(&v))
}

/// System UI language tag, if known.
///
/// Windows: the user's UI language (`GetUserDefaultUILanguage`); locale variables are not used
/// there, since shells like Git Bash set `LANG=en_US.UTF-8` regardless of the UI language.
/// Linux and other Unix: `LC_ALL`, `LC_MESSAGES`, `LANG`. macOS: the same variables, else the
/// user's preferred languages from the system settings (a GUI app started from Finder usually
/// has no locale variables).
pub fn system_language() -> Option<String> {
    #[cfg(windows)]
    {
        windows_ui_language()
    }
    #[cfg(target_os = "macos")]
    {
        from_posix_env(|k| std::env::var(k).ok()).or_else(macos_preferred_language)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        from_posix_env(|k| std::env::var(k).ok())
    }
}

/// First usable tag of the user's preferred languages (`CFLocaleCopyPreferredLanguages`:
/// `ru-RU`, `en-US`, `zh-Hans-CN`, ...). Not verified on macOS yet.
#[cfg(target_os = "macos")]
fn macos_preferred_language() -> Option<String> {
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFLocaleCopyPreferredLanguages() -> CFArrayRef;
    }
    // SAFETY: the function returns a new (+1) CFArray of CFString or NULL; the wrapper takes
    // ownership and releases it on drop.
    let langs: CFArray<CFString> = unsafe {
        let r = CFLocaleCopyPreferredLanguages();
        if r.is_null() {
            return None;
        }
        CFArray::wrap_under_create_rule(r)
    };
    let found = langs.iter().find_map(|s| normalize(&s.to_string()));
    found
}

#[cfg(windows)]
fn windows_ui_language() -> Option<String> {
    use windows_sys::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};
    // LOCALE_NAME_MAX_LENGTH
    let mut buf = [0u16; 85];
    // SAFETY: plain Win32 calls; `buf` outlives the call and its length is passed.
    let n = unsafe {
        let langid = GetUserDefaultUILanguage();
        LCIDToLocaleName(u32::from(langid), buf.as_mut_ptr(), buf.len() as i32, 0)
    };
    if n <= 1 {
        return None;
    }
    normalize(&String::from_utf16_lossy(&buf[..(n - 1) as usize]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_names() {
        assert_eq!(normalize("ru_RU.UTF-8").as_deref(), Some("ru-RU"));
        assert_eq!(normalize("de_DE@euro").as_deref(), Some("de-DE"));
        assert_eq!(normalize("en-us").as_deref(), Some("en-US"));
        assert_eq!(normalize("zh-hans-cn").as_deref(), Some("zh-Hans-CN"));
        assert_eq!(normalize("ru").as_deref(), Some("ru"));
        assert_eq!(normalize("C.UTF-8"), None);
        assert_eq!(normalize("POSIX"), None);
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("../etc"), None);
    }

    #[test]
    fn posix_priority() {
        let env = |vals: &'static [(&'static str, &'static str)]| {
            move |k: &str| vals.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
        };
        assert_eq!(
            from_posix_env(env(&[("LANG", "en_US.UTF-8"), ("LC_MESSAGES", "ru_RU.UTF-8")])).as_deref(),
            Some("ru-RU")
        );
        assert_eq!(
            from_posix_env(env(&[("LC_ALL", "de_DE"), ("LC_MESSAGES", "ru_RU")])).as_deref(),
            Some("de-DE")
        );
        assert_eq!(from_posix_env(env(&[("LC_ALL", ""), ("LANG", "C")])), None);
        assert_eq!(from_posix_env(env(&[])), None);
    }

    #[test]
    fn system_language_does_not_panic() {
        if let Some(l) = system_language() {
            assert_eq!(normalize(&l).as_deref(), Some(l.as_str()));
        }
    }
}
