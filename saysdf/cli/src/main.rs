//! saysdfTerm: the `saysdf` binary — TUI tools (design code-viewer D2).
//!
//! `--help` is English only: clap takes it from the doc comments at compile time (i18n plan D11).

mod cmd;
mod db;
mod editor;
mod files;
mod form;
mod git;
mod keys;
mod log;
mod mon;
mod pager;
mod sidebar;
mod ssh;
mod watch;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "saysdf", version, about = "saysdfTerm TUI tools", disable_help_subcommand = true)]
struct Cli {
    /// UI language: a tag (`en`, `ru`, `pt-BR`) or `auto`; overrides SAYSDF_LANG
    #[arg(long, global = true, value_name = "TAG", value_parser = parse_lang)]
    lang: Option<String>,
    #[command(subcommand)]
    command: Command,
}

/// `--lang` accepts what `SAYSDF_LANG` accepts: `auto` (or empty) or a language tag that
/// `saysdf_i18n::detect::normalize` understands.
fn parse_lang(s: &str) -> Result<String, String> {
    let t = s.trim();
    if t.is_empty() || t.eq_ignore_ascii_case(saysdf_i18n::AUTO) {
        return Ok(saysdf_i18n::AUTO.to_string());
    }
    saysdf_i18n::detect::normalize(t)
        .ok_or_else(|| "not a language tag (examples: en, ru, pt-BR, auto)".to_string())
}

#[derive(Subcommand)]
enum Command {
    /// View a file with syntax highlighting
    View(cmd::view::ViewArgs),
    /// Markdown preview: formatting, tables, code highlighting, local images, live reload
    Md(cmd::md::MdArgs),
    /// Mermaid as character graphics: flowchart and sequence, scroll, zoom (+/-), `--print` to stdout
    Mermaid(cmd::mermaid::MermaidArgs),
    /// Diff: git (work tree/index/commits) or two files/folders; side-by-side/inline, stage a hunk
    Diff(cmd::diff::DiffArgs),
    /// Merge: BASE LOCAL REMOTE MERGED (git mergetool) or a file with conflict markers
    Merge(cmd::merge::MergeArgs),
    /// git repository status as one line of JSON (for the saysdf.git status bar)
    GitStatus(cmd::git_status::GitStatusArgs),
    /// A git operation in a pane: hooks and password prompts are visible, a summary at the end
    GitRun(cmd::git_run::GitRunArgs),
    /// SSH connections: `ssh add` — a form to add a host (your existing entries are not changed)
    Ssh(Box<cmd::ssh::SshArgs>),
    /// Load and process monitor (graphs, tree, actions); `--status-file` — background collector for the status bar
    Mon(mon::MonArgs),
    /// Command hints for the shell: `hints complete` (completer), `hints doctor`, `hints list`
    Hints(cmd::hints::HintsArgs),
    /// Database viewer (SQLite, PostgreSQL, MySQL, Dolt): connections, schema, data, queries; read-only by default
    Db(Box<db::DbArgs>),
    /// saysdfTerm help: what it is, keyboard layouts, hotkeys, security; `--print` to stdout
    Help(cmd::help::HelpArgs),
    /// A pick list in the saysdfTerm side panel (the result is the user var SAYSDF_SIDEBAR)
    Pick(cmd::pick::PickArgs),
    /// File browser: tree, git marks, create/rename/delete, open in the viewer
    Files(cmd::files::FilesArgs),
}

fn main() {
    let cli = Cli::parse();
    // `--lang` overrides SAYSDF_LANG, also for the tools this one starts (editor, nested
    // `saysdf`); set before any thread exists.
    if let Some(lang) = &cli.lang {
        std::env::set_var(saysdf_i18n::LANG_ENV, lang);
    }
    // UI language and user catalogs: read once here, not lazily by the first lookup
    // (security review 2026-10-21-i18n, L3). Broken catalogs are skipped; their warnings would
    // only clutter TUI output. Argument parsing above does no lookups (`--help` is English).
    let _ = saysdf_i18n::init_from_env();
    let code = match cli.command {
        Command::View(args) => cmd::view::run(args),
        Command::Md(args) => cmd::md::run(args),
        Command::Mermaid(args) => cmd::mermaid::run(args),
        Command::Diff(args) => cmd::diff::run(args),
        Command::Merge(args) => cmd::merge::run(args),
        Command::GitStatus(args) => cmd::git_status::run(args),
        Command::GitRun(args) => cmd::git_run::run(args),
        Command::Ssh(args) => cmd::ssh::run(*args),
        Command::Mon(args) => mon::run(args),
        Command::Hints(args) => cmd::hints::run(args),
        Command::Db(args) => db::run(*args),
        Command::Help(args) => cmd::help::run(args),
        Command::Pick(args) => cmd::pick::run(args),
        Command::Files(args) => cmd::files::run(args),
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_option_is_global_and_validated() {
        let lang = |args: &[&str]| Cli::try_parse_from(args).map(|c| c.lang);
        assert_eq!(lang(&["saysdf", "--lang", "ru", "git-status", "--json"]).unwrap().as_deref(), Some("ru"));
        assert_eq!(lang(&["saysdf", "git-status", "--lang", "pt_br", "."]).unwrap().as_deref(), Some("pt-BR"));
        assert_eq!(lang(&["saysdf", "--lang", "AUTO", "hints", "list"]).unwrap().as_deref(), Some("auto"));
        assert_eq!(lang(&["saysdf", "hints", "list"]).unwrap(), None);
        assert!(lang(&["saysdf", "--lang", "../etc", "hints", "list"]).is_err());
        assert!(lang(&["saysdf", "--lang", "C", "hints", "list"]).is_err());
    }
}
