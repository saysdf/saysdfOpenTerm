//! Command completion engine (change command-hints, design D2/D3; prompt-suggest D3).
//!
//! CLI specs are a JSON subset of the withfig/autocomplete schema (`saysdf/hints/specs/*.json`, embedded):
//! `name` (a string or an array: name + aliases), `description`, `subcommands`, `options{name,
//! description, args, isPersistent}`, `args{name, template, generators, suggestions, isOptional,
//! isVariadic}`. Dynamic candidates come only from the allowlist [`Gen`] (`filepaths`, `folders`,
//! `git-branches`, `git-remotes`, `git-tags`); `script` and other generators of the specs are ignored.
//! git runs in safe mode (`Git::safe`, no hooks/fsmonitor) with a 500 ms timeout.
//! Candidates with control characters (C0, C1, DEL) are dropped: specs, git output and file
//! names are untrusted data.
pub mod executables;
pub mod git;
pub mod history;
pub mod suggest;

use crate::git::Git;
use clap::ValueEnum;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Embedded specs: command name -> JSON.
pub const SPECS: &[(&str, &str)] = &[
    ("git", include_str!("../specs/git.json")),
    ("cargo", include_str!("../specs/cargo.json")),
    ("docker", include_str!("../specs/docker.json")),
    ("kubectl", include_str!("../specs/kubectl.json")),
    ("npm", include_str!("../specs/npm.json")),
];

/// Timeout of generators that call git (spec: at most 500 ms).
pub const GEN_TIMEOUT: Duration = Duration::from_millis(500);
/// Limit of path candidates.
const MAX_PATHS: usize = 300;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Shell {
    #[value(alias = "powershell")]
    Pwsh,
    Bash,
    Zsh,
    Fish,
    Plain,
}

// ---------- spec model ----------

#[derive(Debug, Default, Clone)]
pub struct Node {
    pub names: Vec<String>,
    pub description: String,
    pub subcommands: Vec<Node>,
    pub options: Vec<Opt>,
    pub args: Vec<ArgSpec>,
}

#[derive(Debug, Default, Clone)]
pub struct Opt {
    pub names: Vec<String>,
    pub description: String,
    pub args: Vec<ArgSpec>,
    pub persistent: bool,
}

#[derive(Debug, Default, Clone)]
pub struct ArgSpec {
    pub name: String,
    pub generators: Vec<Gen>,
    pub suggestions: Vec<String>,
    pub optional: bool,
    pub variadic: bool,
}

/// Allowlist of generators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gen {
    FilePaths,
    Folders,
    GitBranches,
    GitRemotes,
    GitTags,
}

impl Gen {
    pub fn from_name(s: &str) -> Option<Gen> {
        Some(match s {
            "filepaths" => Gen::FilePaths,
            "folders" => Gen::Folders,
            "git-branches" => Gen::GitBranches,
            "git-remotes" => Gen::GitRemotes,
            "git-tags" => Gen::GitTags,
            _ => return None,
        })
    }
}

fn strs(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => o.get("name").and_then(Value::as_str).map(str::to_owned),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn items(v: Option<&Value>) -> Vec<&Value> {
    match v {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(o @ Value::Object(_)) => vec![o],
        _ => Vec::new(),
    }
}

fn desc(v: &Value) -> String {
    v.get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn parse_arg(v: &Value) -> ArgSpec {
    let mut gens: Vec<Gen> = strs(v.get("template"))
        .iter()
        .filter_map(|t| Gen::from_name(t))
        .filter(|g| matches!(g, Gen::FilePaths | Gen::Folders))
        .collect();
    let g = v.get("generators");
    let list: Vec<&Value> = match g {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(x) => vec![x],
        None => Vec::new(),
    };
    for item in list {
        // a string from the allowlist or an object { "saysdf": "<name>" }; `script`/`custom` are ignored
        let name = match item {
            Value::String(s) => Some(s.as_str()),
            Value::Object(o) => o.get("saysdf").and_then(Value::as_str),
            _ => None,
        };
        if let Some(gen) = name.and_then(Gen::from_name) {
            if !gens.contains(&gen) {
                gens.push(gen);
            }
        }
    }
    ArgSpec {
        name: v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        generators: gens,
        suggestions: strs(v.get("suggestions")),
        optional: v
            .get("isOptional")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        variadic: v
            .get("isVariadic")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn parse_node(v: &Value) -> Node {
    Node {
        names: strs(v.get("name")),
        description: desc(v),
        subcommands: items(v.get("subcommands"))
            .into_iter()
            .map(parse_node)
            .collect(),
        options: items(v.get("options"))
            .into_iter()
            .map(|o| Opt {
                names: strs(o.get("name")),
                description: desc(o),
                args: items(o.get("args")).into_iter().map(parse_arg).collect(),
                persistent: o
                    .get("isPersistent")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
            .collect(),
        args: items(v.get("args")).into_iter().map(parse_arg).collect(),
    }
}

/// Parses a JSON spec.
pub fn parse_spec(text: &str) -> Result<Node, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let n = parse_node(&v);
    if n.names.is_empty() {
        return Err("spec without name".into());
    }
    Ok(n)
}

/// Command name from the first word: `C:\x\git.exe` -> `git`.
pub fn command_name(word: &str) -> String {
    let p = Path::new(word);
    let stem = if p
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe") || e.eq_ignore_ascii_case("cmd"))
    {
        p.file_stem()
    } else {
        p.file_name()
    };
    let s = stem
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

fn parsed_specs() -> &'static [(&'static str, Node)] {
    static PARSED: std::sync::OnceLock<Vec<(&'static str, Node)>> = std::sync::OnceLock::new();
    PARSED.get_or_init(|| {
        SPECS
            .iter()
            .filter_map(|(n, t)| parse_spec(t).ok().map(|node| (*n, node)))
            .collect()
    })
}

pub fn spec_for(cmd: &str) -> Option<&'static Node> {
    parsed_specs()
        .iter()
        .find(|(n, _)| *n == cmd)
        .map(|(_, node)| node)
}

// ---------- completion ----------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub value: String,
    pub description: String,
}

pub struct GenEnv {
    pub cwd: PathBuf,
    pub timeout: Duration,
}

fn is_disabled(cmd: &str, disabled: &str) -> bool {
    disabled
        .split([',', ' ', ';'])
        .map(str::trim)
        .any(|d| d == "*" || d == "1" || d.eq_ignore_ascii_case(cmd))
}

/// Completion by words (the first is the command, the last is the one being completed).
pub fn complete_words(words: &[String], env: &GenEnv, disabled: &str) -> Vec<Candidate> {
    let Some(first) = words.first() else {
        return Vec::new();
    };
    let cmd = command_name(first);
    if is_disabled(&cmd, disabled) {
        return Vec::new();
    }
    match spec_for(&cmd) {
        Some(spec) => complete(spec, words, env),
        None => Vec::new(),
    }
}

fn visible_opts<'a>(path: &[&'a Node]) -> Vec<&'a Opt> {
    let mut out: Vec<&Opt> = Vec::new();
    let last = path.len() - 1;
    for (i, n) in path.iter().enumerate().rev() {
        for o in &n.options {
            if i == last || o.persistent {
                out.push(o);
            }
        }
    }
    out
}

fn find_opt<'a>(path: &[&'a Node], name: &str) -> Option<&'a Opt> {
    visible_opts(path)
        .into_iter()
        .find(|o| o.names.iter().any(|n| n == name))
}

/// Walks the spec tree and returns candidates for the last word (without control characters).
pub fn complete(spec: &Node, words: &[String], env: &GenEnv) -> Vec<Candidate> {
    without_control_chars(complete_unfiltered(spec, words, env))
}

fn complete_unfiltered(spec: &Node, words: &[String], env: &GenEnv) -> Vec<Candidate> {
    if words.len() < 2 {
        return Vec::new(); // completing the command itself is the job of the shell
    }
    let (cur, done) = words[1..].split_last().unwrap();
    let mut path: Vec<&Node> = vec![spec];
    let mut pending: Option<&ArgSpec> = None;
    let mut pos = 0usize;
    let mut dashdash = false;
    for w in done {
        if pending.take().is_some() {
            continue;
        }
        if !dashdash && w == "--" {
            dashdash = true;
            continue;
        }
        if !dashdash && w.len() > 1 && w.starts_with('-') {
            if w.contains('=') {
                continue; // --opt=value
            }
            if let Some(o) = find_opt(&path, w) {
                pending = o.args.first().filter(|a| !a.optional);
                continue;
            }
            if !w.starts_with("--") {
                // -abc: stacked flags; a flag with an argument takes the rest or the next word
                let chars: Vec<char> = w[1..].chars().collect();
                for (i, c) in chars.iter().enumerate() {
                    match find_opt(&path, &format!("-{c}")) {
                        Some(o) if o.args.first().is_some_and(|a| !a.optional) => {
                            if i == chars.len() - 1 {
                                pending = o.args.first();
                            }
                            break;
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
            }
            continue;
        }
        let node = *path.last().unwrap();
        if pos == 0 && !dashdash {
            if let Some(sub) = node
                .subcommands
                .iter()
                .find(|s| s.names.iter().any(|n| n == w))
            {
                path.push(sub);
                continue;
            }
        }
        pos += 1;
    }

    let mut out: Vec<Candidate> = Vec::new();
    if let Some(a) = pending {
        return arg_candidates(a, cur, "", env);
    }
    if !dashdash && cur.starts_with('-') {
        if let Some((name, val)) = cur.split_once('=') {
            if let Some(a) = find_opt(&path, name).and_then(|o| o.args.first()) {
                return arg_candidates(a, val, &format!("{name}="), env);
            }
            return out;
        }
        for o in visible_opts(&path) {
            for n in &o.names {
                if n.starts_with(cur.as_str()) {
                    push(&mut out, n, &o.description);
                }
            }
        }
        sort_dedup(&mut out);
        return out;
    }
    let node = *path.last().unwrap();
    if pos == 0 && !dashdash {
        for s in &node.subcommands {
            // the main (first) name if it matches; else a matching alias
            if let Some(n) = s.names.iter().find(|n| n.starts_with(cur.as_str())) {
                push(&mut out, n, &s.description);
            }
        }
        sort_dedup(&mut out);
    }
    let arg = node
        .args
        .get(pos)
        .or_else(|| node.args.last().filter(|a| a.variadic));
    if let Some(a) = arg {
        out.extend(arg_candidates(a, cur, "", env));
    }
    out
}

fn push(out: &mut Vec<Candidate>, v: &str, d: &str) {
    out.push(Candidate {
        value: v.to_owned(),
        description: d.to_owned(),
    });
}

fn sort_dedup(v: &mut Vec<Candidate>) {
    v.sort_by(|a, b| a.value.cmp(&b.value));
    v.dedup_by(|a, b| a.value == b.value);
}

fn arg_candidates(a: &ArgSpec, cur: &str, prefix: &str, env: &GenEnv) -> Vec<Candidate> {
    let mut out = Vec::new();
    for s in &a.suggestions {
        if s.starts_with(cur) {
            push(&mut out, &format!("{prefix}{s}"), &a.name);
        }
    }
    for g in &a.generators {
        for mut c in run_generator(*g, cur, env) {
            c.value = format!("{prefix}{}", c.value);
            out.push(c);
        }
    }
    out
}

/// Runs an allowlisted generator (read only, git with a timeout), without control characters.
pub fn run_generator(g: Gen, cur: &str, env: &GenEnv) -> Vec<Candidate> {
    without_control_chars(generate(g, cur, env))
}

fn has_control_chars(c: &Candidate) -> bool {
    c.value
        .chars()
        .chain(c.description.chars())
        .any(char::is_control)
}

fn without_control_chars(mut cands: Vec<Candidate>) -> Vec<Candidate> {
    cands.retain(|c| !has_control_chars(c));
    cands
}

fn generate(g: Gen, cur: &str, env: &GenEnv) -> Vec<Candidate> {
    match g {
        Gen::FilePaths => paths(cur, &env.cwd, false),
        Gen::Folders => paths(cur, &env.cwd, true),
        Gen::GitBranches => git_lines(
            env,
            &[
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads",
                "refs/remotes",
            ],
            cur,
            "branch",
        ),
        Gen::GitRemotes => git_lines(env, &["remote"], cur, "remote"),
        Gen::GitTags => git_lines(
            env,
            &["for-each-ref", "--format=%(refname:short)", "refs/tags"],
            cur,
            "tag",
        ),
    }
}

fn git_lines(env: &GenEnv, args: &[&str], cur: &str, what: &str) -> Vec<Candidate> {
    let out = match Git::safe()
        .dir(&env.cwd)
        .timeout(Some(env.timeout))
        .run(args)
    {
        Ok(o) if o.success() => o.stdout_str(),
        _ => return Vec::new(),
    };
    let mut v: Vec<Candidate> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.ends_with("/HEAD") && l.starts_with(cur))
        .map(|l| Candidate {
            value: l.to_owned(),
            description: what.to_owned(),
        })
        .collect();
    v.dedup_by(|a, b| a.value == b.value);
    v
}

fn is_sep(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

fn paths(cur: &str, cwd: &Path, dirs_only: bool) -> Vec<Candidate> {
    let split = cur
        .char_indices()
        .rfind(|(_, c)| is_sep(*c))
        .map(|(i, _)| i + 1);
    let (dir_part, file_prefix) = match split {
        Some(i) => cur.split_at(i),
        None => ("", cur),
    };
    let sep = match dir_part.chars().last() {
        Some(c) => c,
        None if cfg!(windows) => '\\',
        None => '/',
    };
    let dir = if dir_part.is_empty() {
        cwd.to_path_buf()
    } else {
        cwd.join(dir_part)
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let show_hidden = file_prefix.starts_with('.');
    let lower = file_prefix.to_lowercase();
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && !show_hidden {
            continue;
        }
        let ok = if cfg!(windows) {
            name.to_lowercase().starts_with(&lower)
        } else {
            name.starts_with(file_prefix)
        };
        if !ok {
            continue;
        }
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false) || e.path().is_dir();
        if dirs_only && !is_dir {
            continue;
        }
        let value = if is_dir {
            format!("{dir_part}{name}{sep}")
        } else {
            format!("{dir_part}{name}")
        };
        out.push(Candidate {
            value,
            description: if is_dir { "folder" } else { "file" }.to_owned(),
        });
        if out.len() >= MAX_PATHS {
            break;
        }
    }
    out.sort_by(|a, b| a.value.cmp(&b.value));
    out
}

// ---------- input line ----------

/// Splits the line into words (quotes; `\` escapes if `backslash`); a trailing space gives
/// an empty word to complete; the last command after `|`, `;`, `&&`, `||` is taken.
pub fn split_line(line: &str, backslash: bool) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some('"') if backslash && c == '\\' => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            Some(_) => cur.push(c),
            None => {
                if c.is_whitespace() {
                    if in_word {
                        words.push(std::mem::take(&mut cur));
                        in_word = false;
                    }
                    continue;
                }
                in_word = true;
                match c {
                    '\'' | '"' => quote = Some(c),
                    '\\' if backslash => {
                        if let Some(n) = chars.next() {
                            cur.push(n);
                        }
                    }
                    '|' | ';' | '&' if cur.is_empty() => {
                        while chars.peek().is_some_and(|n| matches!(n, '|' | '&')) {
                            chars.next();
                        }
                        words.clear();
                        in_word = false;
                    }
                    _ => cur.push(c),
                }
            }
        }
    }
    if in_word || quote.is_some() {
        words.push(cur);
    } else {
        words.push(String::new());
    }
    words
}

// ---------- output ----------

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn pwsh_quote(v: &str) -> String {
    let special = |c: char| c.is_whitespace() || "'\"$`{}();,|&@#<>".contains(c);
    if v.is_empty() || v.chars().any(special) {
        format!("'{}'", v.replace('\'', "''"))
    } else {
        v.to_owned()
    }
}

fn bash_escape(v: &str) -> String {
    let mut s = String::new();
    for c in v.chars() {
        if c.is_whitespace() || "'\"\\$`!&;|()<>*?[]{}#".contains(c) {
            s.push('\\');
        }
        s.push(c);
    }
    s
}

/// Per-shell format: pwsh `text\tvalue\tdescription`, fish/plain `value\tdescription`,
/// zsh `value:description` for `_describe`, bash only the values (escaped).
pub fn format(shell: Shell, cands: &[Candidate], cur: &str) -> String {
    let mut out = String::new();
    for c in cands {
        let v = clean(&c.value);
        let d = clean(&c.description);
        let line = match shell {
            Shell::Pwsh => {
                let tip = if d.is_empty() { v.clone() } else { d };
                format!("{}\t{}\t{}", pwsh_quote(&v), v, tip)
            }
            Shell::Bash => {
                // bash replaces only the part of the word after the last `=`/`:` (COMP_WORDBREAKS)
                let cut = cur.rfind(['=', ':']).map(|i| i + 1).unwrap_or(0);
                let v = if cut > 0 && v.len() >= cut && v.is_char_boundary(cut) {
                    &v[cut..]
                } else {
                    &v
                };
                bash_escape(v)
            }
            Shell::Zsh => {
                let v = v.replace('\\', "\\\\").replace(':', "\\:");
                if d.is_empty() {
                    v
                } else {
                    format!("{v}:{d}")
                }
            }
            Shell::Fish | Shell::Plain => {
                if d.is_empty() {
                    v
                } else {
                    format!("{v}\t{d}")
                }
            }
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> GenEnv {
        GenEnv {
            cwd: std::env::temp_dir(),
            timeout: GEN_TIMEOUT,
        }
    }

    fn w(s: &str) -> Vec<String> {
        let mut v: Vec<String> = s.split(' ').map(str::to_owned).collect();
        if s.ends_with(' ') {
            v.pop();
            v.push(String::new());
        }
        v
    }

    fn values(s: &str) -> Vec<String> {
        complete_words(&w(s), &env(), "")
            .into_iter()
            .map(|c| c.value)
            .collect()
    }

    #[test]
    fn all_specs_parse() {
        for (name, text) in SPECS {
            let n = parse_spec(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(n.names[0], *name);
            assert!(!n.subcommands.is_empty(), "{name}");
        }
    }

    #[test]
    fn git_ch() {
        assert_eq!(values("git ch"), ["checkout", "cherry-pick"]);
    }

    #[test]
    fn cargo_b_and_alias() {
        assert_eq!(values("cargo b"), ["bench", "build"]);
        let c = complete_words(&w("cargo build --re"), &env(), "");
        assert_eq!(c[0].value, "--release");
        assert!(c[0].description.contains("release mode"));
        // a subcommand alias and a persistent root flag
        assert!(values("cargo b --of").contains(&"--offline".to_owned()));
    }

    #[test]
    fn git_commit_am_flag_with_description() {
        let c = complete_words(&w("git commit --am"), &env(), "");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].value, "--amend");
        assert!(c[0].description.starts_with("Replace the tip"));
    }

    #[test]
    fn option_args_and_combined_flags() {
        // option argument: suggestions, and `--opt=` with a prefix
        assert_eq!(values("cargo --color a"), ["auto", "always"]);
        assert_eq!(values("cargo --color=n"), ["--color=never"]);
        // -m takes the message, then commit flags again
        assert!(values("git commit -m msg --am").contains(&"--amend".to_owned()));
        // -am: -a is a flag, -m takes an argument, so the next word is taken as the message
        assert!(values("git commit -am msg --am").contains(&"--amend".to_owned()));
        assert_eq!(values("kubectl get -o j"), ["json"]);
    }

    #[test]
    fn unknown_command_and_disabled() {
        assert!(values("frobnicate x").is_empty());
        assert!(complete_words(&w("kubectl g"), &env(), "kubectl,npm").is_empty());
        assert!(!complete_words(&w("git ch"), &env(), "kubectl").is_empty());
        assert!(values("git").is_empty());
        assert_eq!(command_name("/usr/bin/git"), "git");
        assert_eq!(command_name("git.exe"), "git");
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_with_backslashes_and_any_case() {
        assert_eq!(command_name("C:\\Program Files\\Git\\cmd\\Git.EXE"), "git");
    }

    #[cfg(unix)]
    #[test]
    fn unix_backslash_is_part_of_file_name() {
        assert_eq!(command_name("a\\git"), "a\\git");
    }

    #[test]
    fn generators_allowlist_ignores_script() {
        let spec = parse_spec(
            r#"{"name":"x","args":{"name":"a","generators":[{"script":["rm","-rf","/"]},
                "evil-gen", {"saysdf":"git-branches"}, "filepaths"],
                "template":["folders","history"]}}"#,
        )
        .unwrap();
        assert_eq!(
            spec.args[0].generators,
            [Gen::Folders, Gen::GitBranches, Gen::FilePaths]
        );
        assert_eq!(Gen::from_name("script"), None);
    }

    #[test]
    fn generator_outside_repo_is_empty() {
        let d = tempfile::tempdir().unwrap();
        let e = GenEnv {
            cwd: d.path().to_path_buf(),
            timeout: GEN_TIMEOUT,
        };
        assert!(run_generator(Gen::GitBranches, "", &e).is_empty());
    }

    #[test]
    fn paths_generator() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "").unwrap();
        std::fs::write(d.path().join("мой файл.txt"), "").unwrap(); // i18n-allow-cyrillic
        std::fs::write(d.path().join(".hidden"), "").unwrap();
        let e = GenEnv {
            cwd: d.path().to_path_buf(),
            timeout: GEN_TIMEOUT,
        };
        let all: Vec<String> = run_generator(Gen::FilePaths, "", &e)
            .into_iter()
            .map(|c| c.value)
            .collect();
        let sep = if cfg!(windows) { '\\' } else { '/' };
        assert_eq!(
            all,
            [
                "Cargo.toml".to_owned(),
                format!("src{sep}"),
                "мой файл.txt".to_owned() // i18n-allow-cyrillic
            ]
        );
        let dirs: Vec<String> = run_generator(Gen::Folders, "./s", &e)
            .into_iter()
            .map(|c| c.value)
            .collect();
        assert_eq!(dirs, ["./src/"]);
    }

    #[test]
    fn split_line_cases() {
        assert_eq!(split_line("git ch", true), ["git", "ch"]);
        assert_eq!(split_line("git commit ", true), ["git", "commit", ""]);
        assert_eq!(
            split_line("cd x && git 'a b' c\\ d", true),
            ["git", "a b", "c d"]
        );
        assert_eq!(
            split_line("git add C:\\dir\\f", false),
            ["git", "add", "C:\\dir\\f"]
        );
    }

    #[test]
    fn escaping_per_shell() {
        let c = vec![
            Candidate {
                value: "мой файл.txt".into(), // i18n-allow-cyrillic
                description: "file".into(),
            },
            Candidate {
                value: "C:\\a b\\it's".into(),
                description: String::new(),
            },
            Candidate {
                value: "a:b".into(),
                description: "d\tx".into(),
            },
        ];
        assert_eq!(
            format(Shell::Pwsh, &c, ""),
            "'мой файл.txt'\tмой файл.txt\tfile\n'C:\\a b\\it''s'\tC:\\a b\\it's\tC:\\a b\\it's\na:b\ta:b\td x\n" // i18n-allow-cyrillic
        );
        assert_eq!(
            format(Shell::Bash, &c, ""),
            "мой\\ файл.txt\nC:\\\\a\\ b\\\\it\\'s\na:b\n" // i18n-allow-cyrillic
        );
        assert_eq!(
            format(Shell::Zsh, &c, ""),
            "мой файл.txt:file\nC\\:\\\\a b\\\\it's\na\\:b:d x\n" // i18n-allow-cyrillic
        );
        assert_eq!(
            format(Shell::Fish, &c, ""),
            "мой файл.txt\tfile\nC:\\a b\\it's\na:b\td x\n" // i18n-allow-cyrillic
        );
        let eq = vec![Candidate {
            value: "--color=never".into(),
            description: String::new(),
        }];
        assert_eq!(format(Shell::Bash, &eq, "--color=n"), "never\n");
    }

    #[test]
    fn control_chars_are_discarded() {
        for bad in [
            "a\x1b[31m",
            "a\x07",
            "a\x00",
            "a\x7f",
            "a\u{9b}31m",
            "a\u{85}",
            "a\nb",
            "a\tb",
            "a\rb",
        ] {
            let json = serde_json::to_string(bad).unwrap();
            let spec = parse_spec(&format!(
                r#"{{"name":"x","subcommands":[{{"name":{json}}},{{"name":"ab"}},{{"name":"ac","description":{json}}}]}}"#
            ))
            .unwrap();
            let got: Vec<String> = complete(&spec, &w("x a"), &env())
                .into_iter()
                .map(|c| c.value)
                .collect();
            assert_eq!(got, ["ab"], "{bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn control_chars_in_file_names_are_discarded() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("ok.txt"), "").unwrap();
        std::fs::write(d.path().join("o\x1b]0;x\x07.txt"), "").unwrap();
        std::fs::write(d.path().join("o\nk.txt"), "").unwrap();
        let e = GenEnv {
            cwd: d.path().to_path_buf(),
            timeout: GEN_TIMEOUT,
        };
        let got: Vec<String> = run_generator(Gen::FilePaths, "o", &e)
            .into_iter()
            .map(|c| c.value)
            .collect();
        assert_eq!(got, ["ok.txt"]);
    }
}
