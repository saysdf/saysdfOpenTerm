//! `saysdf hints complete|doctor|list` — command hints on the shell side (change command-hints,
//! design D2/D3). The engine is the `saysdf-hints` crate.
//!
//! The input line comes from the `SAYSDF_HINTS_LINE` variable (`--from-env`; for bash —
//! `COMP_LINE`/`COMP_POINT`) or as words after `--`. An unknown command gives empty output, code 0.
use clap::{Args, Subcommand};
use saysdf_hints::{complete_words, format, split_line, GenEnv, Shell, GEN_TIMEOUT, SPECS};
use std::path::PathBuf;

#[derive(Args)]
pub struct HintsArgs {
    #[command(subcommand)]
    cmd: HintsCmd,
}

#[derive(Subcommand)]
enum HintsCmd {
    /// Completion candidates for a line: `saysdf hints complete --shell pwsh -- git ch`
    Complete(CompleteArgs),
    /// What is enabled and what is missing (completers, inline suggestion from history)
    Doctor,
    /// Commands that have specs
    List,
}

#[derive(Args)]
struct CompleteArgs {
    /// Output format
    #[arg(long, value_enum, default_value = "plain")]
    shell: Shell,
    /// Take the line from SAYSDF_HINTS_LINE (or COMP_LINE/COMP_POINT for bash `complete -C`)
    #[arg(long)]
    from_env: bool,
    /// Words of the command line; the last one is completed (may be empty)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    words: Vec<String>,
}

pub fn run(args: HintsArgs) -> i32 {
    match args.cmd {
        HintsCmd::Complete(a) => {
            let words = if a.from_env {
                words_from_env(a.shell)
            } else {
                a.words.clone()
            };
            let env = GenEnv {
                cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                timeout: GEN_TIMEOUT,
            };
            let disabled = std::env::var("SAYSDF_HINTS_DISABLED").unwrap_or_default();
            let cands = complete_words(&words, &env, &disabled);
            let cur = words.last().map(String::as_str).unwrap_or("");
            print!("{}", format(a.shell, &cands, cur));
            0
        }
        HintsCmd::Doctor => {
            print!("{}", doctor_report(&|k| std::env::var(k).ok()));
            0
        }
        HintsCmd::List => {
            for (name, _) in SPECS {
                println!("{name}");
            }
            0
        }
    }
}

/// Words from the environment: `SAYSDF_HINTS_LINE` (text before the cursor) or `COMP_LINE`/`COMP_POINT`.
fn words_from_env(shell: Shell) -> Vec<String> {
    let line = match std::env::var("SAYSDF_HINTS_LINE") {
        Ok(l) => l,
        Err(_) => {
            let l = std::env::var("COMP_LINE").unwrap_or_default();
            match std::env::var("COMP_POINT")
                .ok()
                .and_then(|p| p.parse::<usize>().ok())
            {
                Some(p) if p <= l.len() && l.is_char_boundary(p) => l[..p].to_owned(),
                _ => l,
            }
        }
    };
    split_line(&line, shell != Shell::Pwsh)
}

/// Report of `saysdf hints doctor` (the variables are set by the shell-integration scripts).
pub fn doctor_report(get: &dyn Fn(&str) -> Option<String>) -> String {
    let mut s = String::from("saysdf hints doctor\n");
    let names: Vec<&str> = SPECS.iter().map(|(n, _)| *n).collect();
    s.push_str(&format!("specs:      {}\n", names.join(" ")));
    let enabled = get("SAYSDFTERM_HINTS");
    let si = get("SAYSDFTERM_SHELL_INTEGRATION");
    let on = match (enabled.as_deref(), si.as_deref()) {
        (Some("1"), _) => "yes (SAYSDFTERM_HINTS=1)".to_owned(),
        (Some(v), _) => format!("no (SAYSDFTERM_HINTS={v})"),
        (None, Some(_)) => "no (module saysdf.hints is off)".to_owned(),
        (None, None) => "yes (outside saysdfTerm, scripts sourced manually)".to_owned(),
    };
    s.push_str(&format!("enabled:    {on}\n"));
    let dis = get("SAYSDF_HINTS_DISABLED").filter(|v| !v.is_empty());
    s.push_str(&format!("disabled:   {}\n", dis.as_deref().unwrap_or("-")));
    match get("SAYSDF_HINTS_ACTIVE").filter(|v| !v.is_empty()) {
        Some(a) => s.push_str(&format!(
            "completers: {a} ({})\n",
            get("SAYSDF_HINTS_SHELL").unwrap_or_default()
        )),
        None => s.push_str(
            "completers: not registered in this shell (shell-integration scripts not loaded?)\n",
        ),
    }
    match get("SAYSDF_HINTS_INLINE").filter(|v| !v.is_empty()) {
        Some(v) if v.starts_with("unavailable") => s.push_str(&format!("inline:     {v}\n")),
        Some(v) => s.push_str(&format!("inline:     {v}\n")),
        None => s.push_str("inline:     unknown\n"),
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_format() {
        let r = doctor_report(&|k| match k {
            "SAYSDFTERM_HINTS" => Some("1".into()),
            "SAYSDF_HINTS_ACTIVE" => Some("git,cargo".into()),
            "SAYSDF_HINTS_SHELL" => Some("zsh".into()),
            "SAYSDF_HINTS_INLINE" => Some("unavailable: install zsh-autosuggestions".into()),
            _ => None,
        });
        assert!(r.contains("enabled:    yes"));
        assert!(r.contains("completers: git,cargo (zsh)"));
        assert!(r.contains("inline:     unavailable: install zsh-autosuggestions"));
        assert!(r.contains("specs:      git cargo docker kubectl npm"));
    }
}
