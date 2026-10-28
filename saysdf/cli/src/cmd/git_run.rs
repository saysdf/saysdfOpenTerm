//! `saysdf git-run [--cwd <dir>] -- <git args>` — a git operation in a split (git-integration D5):
//! the TTY is inherited (hooks and password prompts are visible), a summary at the end; on error
//! it waits for Enter.
use crate::git::{self, Git};
use clap::Args;
use std::io::{BufRead, Write};
use std::path::PathBuf;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-git");

#[derive(Args)]
pub struct GitRunArgs {
    /// Working directory
    #[arg(long)]
    cwd: Option<PathBuf>,
    /// Do not wait for Enter on error
    #[arg(long)]
    no_wait: bool,
    /// git arguments
    #[arg(last = true, required = true)]
    args: Vec<String>,
}

pub fn run(args: GitRunArgs) -> i32 {
    let mut g = Git::new();
    if let Some(d) = &args.cwd {
        g = g.dir(d);
    }
    println!("$ git {}", args.args.join(" "));
    let code = match g.command(&args.args).status() {
        Ok(st) => st.code().unwrap_or(1),
        Err(e) => {
            eprintln!("saysdf git-run: {}", git::Error::from(e));
            2
        }
    };
    if code == 0 {
        let cmd = args.args.first().map(String::as_str).unwrap_or("");
        println!("\n[{}]", I18N.f("git {command}: done", &[("command", &cmd)]));
        std::thread::sleep(std::time::Duration::from_millis(1200));
    } else {
        println!("\n[{}]", I18N.f("git exited with code {code}", &[("code", &code)]));
        if !args.no_wait {
            print!("{}", I18N.t("Press Enter to close…"));
            let _ = std::io::stdout().flush();
            let mut s = String::new();
            let _ = std::io::stdin().lock().read_line(&mut s);
        }
    }
    code
}
