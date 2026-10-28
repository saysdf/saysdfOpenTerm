use crate::executables::executables;
use crate::history::{self, SessionHistory};
use crate::{
    bash_escape, command_name, complete, pwsh_quote, run_generator, spec_for, split_line, Gen,
    GenEnv, Node, Shell, GEN_TIMEOUT,
};
use std::collections::HashSet;
use std::path::PathBuf;

pub const DEFAULT_MAX_ROWS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    History,
    Spec,
    File,
    Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub display: String,
    pub insert: String,
    pub description: String,
    pub source: Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sources {
    pub history: bool,
    pub specs: bool,
    pub generators: bool,
    pub files: bool,
    pub commands: bool,
}

impl Default for Sources {
    fn default() -> Self {
        Sources {
            history: true,
            specs: true,
            generators: true,
            files: true,
            commands: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Context<'a> {
    pub shell: Shell,
    pub cwd: Option<PathBuf>,
    pub history_file: Option<PathBuf>,
    pub path_var: Option<String>,
    pub session: &'a SessionHistory,
    pub local_sources: bool,
    pub sources: Sources,
    pub max_rows: usize,
}

impl<'a> Context<'a> {
    pub fn new(shell: Shell, session: &'a SessionHistory) -> Self {
        Context {
            shell,
            cwd: None,
            history_file: None,
            path_var: None,
            session,
            local_sources: false,
            sources: Sources::default(),
            max_rows: DEFAULT_MAX_ROWS,
        }
    }
}

fn open_quote(line: &str, backslash: bool) -> Option<char> {
    let mut quote = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some('"') if backslash && c == '\\' => {
                chars.next();
            }
            Some(_) => {}
            None if backslash && c == '\\' => {
                chars.next();
            }
            None if c == '\'' || c == '"' => quote = Some(c),
            None => {}
        }
    }
    quote
}

fn pwsh_backtick_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_whitespace() || "'\"$`{}();,|&@#<>".contains(c) {
            out.push('`');
        }
        out.push(c);
    }
    out
}

fn escape_suffix(
    shell: Shell,
    quote: Option<char>,
    word_empty: bool,
    suffix: &str,
) -> Option<String> {
    if let Some(q) = quote {
        let unsafe_in_quote = |c: char| c == q || (shell != Shell::Pwsh && "\\$`".contains(c));
        return (!suffix.chars().any(unsafe_in_quote)).then(|| suffix.to_owned());
    }
    Some(match shell {
        Shell::Pwsh if word_empty => pwsh_quote(suffix),
        Shell::Pwsh => pwsh_backtick_escape(suffix),
        _ => bash_escape(suffix),
    })
}

fn without_generators(node: &mut Node, keep: &dyn Fn(Gen) -> bool) {
    for a in node.args.iter_mut() {
        a.generators.retain(|g| keep(*g));
    }
    for o in node.options.iter_mut() {
        for a in o.args.iter_mut() {
            a.generators.retain(|g| keep(*g));
        }
    }
    for s in node.subcommands.iter_mut() {
        without_generators(s, keep);
    }
}

struct Collector<'a> {
    out: Vec<Suggestion>,
    seen: HashSet<String>,
    max: usize,
    ctx: &'a Context<'a>,
    quote: Option<char>,
    cur: &'a str,
}

impl Collector<'_> {
    fn full(&self) -> bool {
        self.out.len() >= self.max
    }

    fn push(&mut self, display: &str, insert: String, description: &str, source: Source) {
        if self.full()
            || insert.is_empty()
            || insert.chars().any(char::is_control)
            || display.chars().any(char::is_control)
            || description.chars().any(char::is_control)
            || !self.seen.insert(insert.clone())
        {
            return;
        }
        self.out.push(Suggestion {
            display: display.to_owned(),
            insert,
            description: description.to_owned(),
            source,
        });
    }

    fn history(&mut self, line: &str, entries: impl Iterator<Item = impl AsRef<str>>) {
        for e in entries {
            if self.full() {
                return;
            }
            let e = e.as_ref();
            if let Some(rest) = e.strip_prefix(line) {
                self.push(e, rest.to_owned(), "", Source::History);
            }
        }
    }

    fn word(&mut self, value: &str, description: &str, source: Source) {
        let Some(suffix) = value.strip_prefix(self.cur) else {
            return;
        };
        if let Some(insert) = escape_suffix(self.ctx.shell, self.quote, self.cur.is_empty(), suffix)
        {
            self.push(value, insert, description, source);
        }
    }
}

pub fn suggest(line: &str, ctx: &Context) -> Vec<Suggestion> {
    if line.trim().is_empty() || line.chars().any(char::is_control) || ctx.max_rows == 0 {
        return Vec::new();
    }
    let backslash = ctx.shell != Shell::Pwsh;
    let words = split_line(line, backslash);
    let cur = words.last().map_or("", String::as_str);
    let mut c = Collector {
        out: Vec::new(),
        seen: HashSet::new(),
        max: ctx.max_rows,
        ctx,
        quote: open_quote(line, backslash),
        cur,
    };
    let local = ctx.local_sources;
    let s = ctx.sources;

    if s.history {
        c.history(line, ctx.session.iter());
        if local {
            let file = ctx
                .history_file
                .clone()
                .or_else(|| history::default_path(ctx.shell));
            if let Some(entries) = file.and_then(|p| history::load(ctx.shell, &p).ok()) {
                c.history(line, entries.iter());
            }
        }
    }

    let gen_env = ctx.cwd.clone().filter(|_| local).map(|cwd| GenEnv {
        cwd,
        timeout: GEN_TIMEOUT,
    });
    let first_word = words.len() == 1;

    if s.specs && !first_word && !c.full() {
        if let Some(spec) = spec_for(&command_name(&words[0])) {
            let mut spec = spec.clone();
            let gens_on = s.generators && gen_env.is_some();
            let keep = |g: Gen| gens_on && (s.files || !matches!(g, Gen::FilePaths | Gen::Folders));
            without_generators(&mut spec, &keep);
            let no_env = GenEnv {
                cwd: PathBuf::new(),
                timeout: GEN_TIMEOUT,
            };
            for cand in complete(&spec, &words, gen_env.as_ref().unwrap_or(&no_env)) {
                let source = match cand.description.as_str() {
                    "file" | "folder" => Source::File,
                    _ => Source::Spec,
                };
                c.word(&cand.value, &cand.description, source);
            }
        }
    }

    let looks_like_path = cur.contains('/') || (cfg!(windows) && cur.contains('\\'));
    if s.files && (!first_word || looks_like_path) && !c.full() {
        if let Some(env) = &gen_env {
            for cand in run_generator(Gen::FilePaths, cur, env) {
                c.word(&cand.value, &cand.description, Source::File);
            }
        }
    }

    if s.commands && local && first_word && !looks_like_path && !cur.is_empty() && !c.full() {
        if let Some(path_var) = &ctx.path_var {
            let names = executables(path_var);
            let start = names.partition_point(|n| n.as_str() < cur);
            for n in names[start..].iter().take_while(|n| n.starts_with(cur)) {
                if c.full() {
                    break;
                }
                c.word(n, "command", Source::Command);
            }
        }
    }

    c.out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn inserts(v: &[Suggestion]) -> Vec<&str> {
        v.iter().map(|s| s.insert.as_str()).collect()
    }

    #[test]
    fn spec_words_are_suffixes() {
        let session = SessionHistory::default();
        let ctx = Context::new(Shell::Bash, &session);
        let got = suggest("cargo b", &ctx);
        assert_eq!(inserts(&got), ["ench", "uild"]);
        assert_eq!(got[1].display, "build");
        let got = suggest("git commit --am", &ctx);
        assert_eq!(inserts(&got), ["end"]);
        assert_eq!(got[0].source, Source::Spec);
        assert!(got[0].description.starts_with("Replace the tip"));
    }

    #[test]
    fn whole_line_history_first_session_above_file() {
        let d = tempfile::tempdir().unwrap();
        let hist = d.path().join("h");
        std::fs::write(&hist, "cargo build --release\ncargo bench\ncargo b\n").unwrap();
        let mut session = SessionHistory::default();
        session.push("cargo build -p x");
        let mut ctx = Context::new(Shell::Bash, &session);
        ctx.local_sources = true;
        ctx.history_file = Some(hist);
        ctx.cwd = Some(d.path().to_path_buf());
        let got = suggest("cargo b", &ctx);
        assert_eq!(
            inserts(&got),
            ["uild -p x", "ench", "uild --release", "uild"]
        );
        assert_eq!(got[0].source, Source::History);
        assert_eq!(got[3].source, Source::Spec);
        ctx.max_rows = 2;
        assert_eq!(suggest("cargo b", &ctx).len(), 2);
    }

    #[test]
    fn reduced_mode_uses_only_session_and_specs_without_generators() {
        let d = tempfile::tempdir().unwrap();
        let hist = d.path().join("h");
        std::fs::write(&hist, "git checkout file-history\n").unwrap();
        std::fs::write(d.path().join("file-x"), "").unwrap();
        let bin = tempfile::tempdir().unwrap();
        std::fs::write(bin.path().join("gitk"), "").unwrap();
        let mut session = SessionHistory::default();
        session.push("git checkout session-branch");
        let mut ctx = Context::new(Shell::Bash, &session);
        ctx.history_file = Some(hist);
        ctx.cwd = Some(d.path().to_path_buf());
        ctx.path_var = Some(bin.path().to_str().unwrap().to_owned());
        assert!(!ctx.local_sources);
        let got = suggest("git checkout ", &ctx);
        assert_eq!(inserts(&got), ["session-branch"]);
        let got = suggest("git checkout f", &ctx);
        assert!(got.is_empty(), "{got:?}");
        assert_eq!(inserts(&suggest("gi", &ctx)), ["t checkout session-branch"]);
        assert_eq!(
            inserts(&suggest("git ch", &ctx)),
            ["eckout session-branch", "eckout", "erry-pick"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn path_commands_for_first_word_only() {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().unwrap();
        for n in ["gitk", "git", "grep"] {
            let p = bin.path().join(n);
            std::fs::write(&p, "").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let session = SessionHistory::default();
        let mut ctx = Context::new(Shell::Bash, &session);
        ctx.local_sources = true;
        ctx.sources.history = false;
        ctx.path_var = Some(bin.path().to_str().unwrap().to_owned());
        let got = suggest("gi", &ctx);
        assert_eq!(inserts(&got), ["t", "tk"]);
        assert_eq!(got[0].source, Source::Command);
        assert!(suggest("ls gi", &ctx)
            .iter()
            .all(|s| s.source != Source::Command));
        assert_eq!(inserts(&suggest("ls | gr", &ctx)), ["ep"]);
    }

    #[test]
    fn file_names_are_escaped_per_shell() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("my file's.txt"), "").unwrap();
        let session = SessionHistory::default();
        let run = |shell, line: &str| {
            let mut ctx = Context::new(shell, &session);
            ctx.local_sources = true;
            ctx.sources.history = false;
            ctx.cwd = Some(d.path().to_path_buf());
            inserts(&suggest(line, &ctx))
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(run(Shell::Bash, "cat my"), ["\\ file\\'s.txt"]);
        assert_eq!(run(Shell::Fish, "cat my\\ f"), ["ile\\'s.txt"]);
        assert_eq!(run(Shell::Bash, "cat \"my"), [" file's.txt"]);
        assert!(run(Shell::Bash, "cat 'my").is_empty());
        assert_eq!(run(Shell::Pwsh, "cat my"), ["` file`'s.txt"]);
        assert_eq!(run(Shell::Pwsh, "cat "), ["'my file''s.txt'"]);
        assert!(run(Shell::Bash, "cat x").is_empty());
    }

    #[test]
    fn only_continuations_and_no_control_chars() {
        let mut session = SessionHistory::default();
        session.push("git status");
        let ctx = Context::new(Shell::Bash, &session);
        assert!(suggest("git status", &ctx)
            .iter()
            .all(|s| !s.insert.is_empty()));
        assert!(suggest("   ", &ctx).is_empty());
        assert!(suggest("git\x1b", &ctx).is_empty());
        for s in suggest("git s", &ctx) {
            let line = format!("git s{}", s.insert);
            assert!(line.starts_with("git s"));
        }
        assert_eq!(suggest("git s", &ctx)[0].insert, "tatus");
    }

    #[test]
    fn fast_with_realistic_history() {
        let d = tempfile::tempdir().unwrap();
        let hist = d.path().join("h");
        let text: String = (0..20_000)
            .map(|i| format!("cargo test -p crate{} --all-targets\n", i % 12_000))
            .collect();
        std::fs::write(&hist, text).unwrap();
        let session = SessionHistory::default();
        let mut ctx = Context::new(Shell::Bash, &session);
        ctx.local_sources = true;
        ctx.history_file = Some(hist);
        ctx.cwd = Some(d.path().to_path_buf());
        ctx.sources.generators = false;
        suggest("x", &ctx);
        let n = 200;
        let t = Instant::now();
        for _ in 0..n {
            assert_eq!(
                suggest("cargo test -p crate1", &ctx).len(),
                DEFAULT_MAX_ROWS
            );
            suggest("cargo zzz", &ctx);
        }
        let per_call = t.elapsed() / (2 * n);
        eprintln!("suggest per call (warm, 10000 history entries): {per_call:?}");
        assert!(per_call.as_millis() < 20, "{per_call:?}");
    }
}
