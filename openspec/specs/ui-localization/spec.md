# ui-localization Specification

## Purpose
Show the fork's user interface (bundled Lua modules, the `saysdf` tools, the render and hints engines) in the user's language: English source strings, translations in embedded and user TOML catalogs, language selection with system detection, placeholders and plurals, per-language menu titles and accelerators, and tests that keep the sources and catalogs in sync.

## Requirements

### Requirement: English source strings with translation catalogs
Every user-visible string of the fork's code (bundled Lua modules, the `saysdf` tools, the render and hints engines) SHALL be written in English in the source and looked up through the localization API with the English text as the key. Translations MUST be TOML data files, one per domain (a Lua module or an area of a Rust crate), at `saysdf/locales/<lang>/<domain>.toml`, embedded in the binaries. A catalog file MUST NOT be able to execute code. English MUST need no catalog; a key without a translation MUST be shown in English.

#### Scenario: Russian UI from the catalog
- **WHEN** the language is `ru` and the quick select "open" mode picks `10.0.0.1`
- **THEN** the toast shows the Russian translation of `Copied, cannot be opened: {text}` from the `quickselect` catalog with `10.0.0.1` in place of `{text}`, the same text as before the migration

#### Scenario: English UI
- **WHEN** the language is `en`
- **THEN** the same toast reads `Copied, cannot be opened: 10.0.0.1`

#### Scenario: Missing translation
- **WHEN** the language is `ru` and a key has no entry in the Russian catalog
- **THEN** the English text is shown and nothing fails

### Requirement: Placeholders and plurals
Strings with variable parts SHALL use named placeholders `{name}`; `{{` and `}}` MUST produce literal braces. Plural strings SHALL be looked up with the English singular and plural forms and a count, and catalogs MUST be able to give a form per CLDR category; Russian MUST distinguish `one`, `few` and `many`. A missing plural form MUST fall back to `other`, then to English.

#### Scenario: Russian plural forms
- **WHEN** a Russian catalog gives the key `"{n} file"` a table with the forms `one`, `few` and `many`
- **THEN** counts 1 and 21 are shown with the `one` form, 2 and 22 with the `few` form, and 5 and 11 with the `many` form, each with the count in place of `{n}`

#### Scenario: Unknown placeholder in a translation
- **WHEN** a catalog entry uses a placeholder its key does not have
- **THEN** the catalog file is rejected as invalid

### Requirement: Language selection
The UI language SHALL be set by `require("saysdf").setup({ language = ... })` with `"auto"` as the default. `auto` MUST use the system UI language (Windows: the user's UI language; Linux and macOS: `LC_ALL`, `LC_MESSAGES`, `LANG` in this order; macOS without them: the first of the user's preferred languages) when a catalog exists for it or for its base language, else English. An explicit language without any catalog MUST fall back to English with one warning in the log. The language MUST be applied before the modules build their labels and MUST follow config reloads.

#### Scenario: Russian Windows
- **WHEN** the Windows UI language is Russian and `language` is not set
- **THEN** the terminal UI is Russian

#### Scenario: Language without a catalog
- **WHEN** `language = "de"` and no German catalog exists
- **THEN** the UI is English and the log has one warning naming `"de"`

#### Scenario: Regional tag
- **WHEN** the system language is `pt-BR` and only a `pt` catalog exists
- **THEN** the `pt` catalog is used

### Requirement: Language of the tools
The terminal SHALL pass the resolved language to the programs it starts in panes through the environment variable `SAYSDF_LANG`, unless the user's config already sets that variable. The `saysdf` tools MUST use `SAYSDF_LANG` when set and otherwise detect the system language the same way as the terminal, so they also work when started by hand.

#### Scenario: Tool in a split
- **WHEN** `language = "en"` on Russian Windows and the user opens the database viewer in a split
- **THEN** the viewer shows English text

#### Scenario: Tool started by hand
- **WHEN** the user runs `saysdf` from a shell outside the terminal on a Russian system
- **THEN** the tool shows Russian text

### Requirement: User translation catalogs
Users SHALL be able to add catalogs at `<config dir>/locales/<lang>/<domain>.toml`: entries override embedded ones key by key, and a new `<lang>` directory adds a language. User catalogs MUST be loaded once per config load, never from disk per UI update. A user file MUST be rejected as a whole, with the problems reported in one warning, when it is larger than 256 KiB, has more than 4096 entries, a key or translation longer than 4096 bytes, a value that is not a string or plural table, control characters other than newline and tab, invalid placeholders, or a file name outside `[a-z0-9_-]`; at most 64 files per language MUST be read. Language names MUST be validated before they are used as directory names. A rejected file MUST NOT stop the terminal from starting or affect other catalogs.

#### Scenario: Override one string
- **WHEN** the user creates `locales/ru/menu.toml` with their own translation of `"View"`
- **THEN** after a config reload the menu bar shows that translation; other Russian strings are unchanged

#### Scenario: Broken user file
- **WHEN** a user catalog contains `"View" = [1, 2]`
- **THEN** that file is ignored, the log has one `saysdf.i18n` warning naming the file, and the terminal starts with the embedded translations

#### Scenario: Path traversal in the language
- **WHEN** `language = "../../etc"`
- **THEN** no directory outside `locales` is read and the UI is English

#### Scenario: New language
- **WHEN** the user creates `locales/eo/menu.toml` and sets `language = "eo"`
- **THEN** the menu titles from that file are shown and other strings are English

### Requirement: Menu titles and accelerators per language
Menus SHALL be identified by the English ids `File`, `Tabs`, `Git`, `Tools`, `View`, `Help`, and the menu bar MUST show their translations. Menu accelerators SHALL stay physical keys independent of the label, and a catalog MAY override the accelerator of a menu entry by its id in the reserved table `_accel`. Accelerators MUST be unique within each menu in every language that has a catalog.

#### Scenario: Russian menu bar
- **WHEN** the language is `ru`
- **THEN** the menu bar shows the Russian translations of `File`, `Tabs`, `Tools`, `View` and `Help` from the `menu` catalog, `Git` stays `Git`, and every entry keeps its accelerator

#### Scenario: Accelerators checked in every language
- **WHEN** the bundled default config is loaded with each language that has a catalog
- **THEN** no menu entry is rejected for a missing or duplicate accelerator

### Requirement: Localization source checks
The test suite SHALL check that every key looked up in the fork's Lua and Rust code has an entry in the Russian catalog of its domain with the same placeholders (plural keys with the forms `one`, `few`, `many`), that every catalog entry is used, and that a Lua module's domain equals its file name. The test suite SHALL also check that the fork's code contains no Cyrillic outside catalogs, keyboard-layout data and lines marked `i18n-allow-cyrillic`, using the same allow-list as the release leak scan (`[scan] cyrillic_allow` and `cyrillic_allow_marker` of `saysdf/public/manifest.toml`).

#### Scenario: New string without translation
- **WHEN** a developer adds `t("Reload")` to a Lua module without a Russian entry
- **THEN** the check fails naming the module catalog and the key

#### Scenario: Russian text in code
- **WHEN** a Russian string or comment is added to a code file outside the allow-list, on a line without the marker
- **THEN** the check fails naming the file and the lines
