//! `saysdf files [<dir>]` — file browser (change file-browser); logic — `crate::files`.

use crate::sidebar;
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct FilesArgs {
    /// cwd sync file (written by saysdfTerm when the active pane's folder changes)
    #[arg(long)]
    pub cwd_file: Option<PathBuf>,
    /// Root folder (the current one by default)
    pub dir: Option<PathBuf>,
}

pub fn run(args: FilesArgs) -> i32 {
    // The token — before any child process (tool-sidebar D3); without it GUI actions are unavailable.
    let token = sidebar::take_token();
    let dir = args.dir.unwrap_or_else(|| PathBuf::from("."));
    match crate::files::run(&dir, token, args.cwd_file) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("saysdf files: {}: {e}", dir.display());
            1
        }
    }
}
