//! `saysdf ssh add` — add an SSH connection (change ssh-connections, design D1/D5).
//! With `--alias` it is non-interactive (scripts, tests), otherwise a ratatui form.
use crate::form::{Field, Session};
use crate::ssh::{self, meta, HostEntry};
use clap::{Args, Subcommand};
use std::borrow::Cow;
use std::path::{Path, PathBuf};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-ssh");

#[derive(Args)]
pub struct SshArgs {
    #[command(subcommand)]
    cmd: SshCmd,
}

#[derive(Subcommand)]
enum SshCmd {
    /// Add a host to ~/.ssh/saysdfterm.conf (+ an Include in ~/.ssh/config after confirmation)
    Add(AddArgs),
}

#[derive(Args)]
pub struct AddArgs {
    /// Alias (`ssh <alias>`): Latin letters, digits, . _ -; with it the form is not shown
    #[arg(long)]
    alias: Option<String>,
    /// HostName (address)
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    user: Option<String>,
    #[arg(long)]
    port: Option<String>,
    /// IdentityFile (path to the key; passwords are not stored)
    #[arg(long)]
    identity: Option<String>,
    /// Paste a command `ssh -i "<key>" user@host [-p N]`: it is parsed into fields, not executed
    #[arg(long)]
    command: Option<String>,
    /// Title in the saysdfTerm list
    #[arg(long)]
    title: Option<String>,
    /// Description (second column of the list)
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    group: Option<String>,
    /// Tab color (a saysdf.tabs palette name or #rrggbb)
    #[arg(long)]
    color: Option<String>,
    /// JSON array (or object) of already known aliases, for duplicate checks
    #[arg(long)]
    existing_file: Option<PathBuf>,
    /// Add the Include to ~/.ssh/config without asking (non-interactive mode)
    #[arg(long)]
    yes_include: bool,
    /// ssh folder instead of ~/.ssh (for trying it in a temporary folder)
    #[arg(long)]
    ssh_dir: Option<PathBuf>,
    /// Metadata file (default <config>/saysdf/ssh-connections.json)
    #[arg(long)]
    meta_file: Option<PathBuf>,
}

pub fn run(args: SshArgs) -> i32 {
    match args.cmd {
        SshCmd::Add(a) => add(a),
    }
}

fn read_known(path: Option<&Path>) -> Result<Vec<String>, String> {
    let Some(p) = path else { return Ok(Vec::new()) };
    let bytes = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(match v {
        serde_json::Value::Array(a) => a.into_iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
        serde_json::Value::Object(o) => o.keys().cloned().collect(),
        _ => Vec::new(),
    })
}

struct Paths {
    own: PathBuf,
    config: PathBuf,
    meta: PathBuf,
}

fn opt(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

/// Field values: 0 alias, 1 host, 2 user, 3 port, 4 identity, 5 title, 6 description, 7 group,
/// 8 color, 9 the pasted ssh command (fills only empty fields; not executed).
fn build(v: &[String; 10]) -> Result<(HostEntry, meta::Meta, Vec<String>), ssh::Error> {
    let mut v = v.clone();
    let mut notes = Vec::new();
    if !v[9].trim().is_empty() {
        let c = ssh::parse_ssh_command(&v[9])?;
        let mut fill = |i: usize, x: Option<String>| {
            if let Some(x) = x.filter(|_| v[i].trim().is_empty()) {
                v[i] = x;
            }
        };
        fill(1, Some(c.host));
        fill(2, c.user);
        fill(3, c.port.map(|p| p.to_string()));
        fill(4, c.identity);
        notes.push(I18N.t("fields filled from the ssh command (it was not run)").into_owned());
    }
    let mut entry = HostEntry {
        alias: v[0].trim().into(),
        hostname: v[1].trim().into(),
        user: opt(&v[2]),
        port: ssh::parse_port(&v[3])?,
        identity: opt(&v[4]),
    };
    notes.extend(entry.normalize());
    entry.validate()?;
    notes.extend(entry.warnings());
    let m = meta::Meta {
        title: v[5].trim().into(),
        description: v[6].trim().into(),
        group: v[7].trim().into(),
        color: v[8].trim().into(),
    };
    m.validate()?;
    Ok((entry, m, notes))
}

fn save(p: &Paths, entry: &HostEntry, m: &meta::Meta, known: &[String]) -> Result<(), ssh::Error> {
    ssh::append_host(&p.own, entry, known)?;
    if !m.is_empty() {
        meta::upsert(&p.meta, &entry.alias, m)?;
    }
    Ok(())
}

fn include_msg(res: Result<Option<Option<PathBuf>>, ssh::Error>) -> String {
    match res {
        Ok(None) => I18N.t("Include already present").into_owned(),
        Ok(Some(Some(b))) => I18N.f("Include added, backup: {path}", &[("path", &b.display())]),
        Ok(Some(None)) => I18N.t("created ~/.ssh/config with the Include").into_owned(),
        Err(e) => e.to_string(),
    }
}

fn not_included(alias: &str) -> String {
    I18N.f(
        "Include not added: `ssh {alias}` will not see the host until ~/.ssh/config starts with the line \"{line}\"",
        &[("alias", &alias), ("line", &ssh::INCLUDE_LINE)],
    )
}

fn add(a: AddArgs) -> i32 {
    let Some(ssh_dir) = a.ssh_dir.clone().or_else(|| ssh::home_dir().map(|h| h.join(".ssh"))) else {
        eprintln!("{}", I18N.t("saysdf ssh add: home folder not found (HOME/USERPROFILE)"));
        return 2;
    };
    let Some(meta_path) = a.meta_file.clone().or_else(meta::default_path) else {
        eprintln!("{}", I18N.t("saysdf ssh add: metadata path not found (--meta-file)"));
        return 2;
    };
    let p = Paths { own: ssh_dir.join(ssh::OWN_FILE), config: ssh_dir.join("config"), meta: meta_path };
    let known = match read_known(a.existing_file.as_deref()) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("{}", I18N.f("saysdf ssh add: --existing-file {error}", &[("error", &e)]));
            return 2;
        }
    };
    let s = |o: &Option<String>| o.clone().unwrap_or_default();
    let values = [
        s(&a.alias),
        s(&a.host),
        s(&a.user),
        s(&a.port),
        s(&a.identity),
        s(&a.title),
        s(&a.description),
        s(&a.group),
        s(&a.color),
        s(&a.command),
    ];
    if a.alias.is_some() {
        return non_interactive(&p, &values, &known, a.yes_include);
    }
    match interactive(&p, values, &known) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{}", I18N.f("saysdf ssh add: {error}", &[("error", &e)]));
            1
        }
    }
}

fn non_interactive(p: &Paths, values: &[String; 10], known: &[String], yes_include: bool) -> i32 {
    let res = build(values).and_then(|(e, m, n)| save(p, &e, &m, known).map(|_| (e, n)));
    let entry = match res {
        Ok((e, notes)) => {
            for n in notes {
                println!("{n}");
            }
            e
        }
        Err(e) => {
            eprintln!("{}", I18N.f("saysdf ssh add: {error}", &[("error", &e)]));
            return 1;
        }
    };
    println!("{}", I18N.f("added Host {alias} to {path}", &[("alias", &entry.alias), ("path", &p.own.display())]));
    if ssh::include_present(&p.config) {
        return 0;
    }
    if yes_include {
        let res = ssh::ensure_include(&p.config, ssh::INCLUDE_LINE);
        let failed = res.is_err();
        println!("{}", include_msg(res));
        return i32::from(failed);
    }
    println!("{}", not_included(&entry.alias));
    0
}

fn field_of(e: &ssh::Error) -> Option<usize> {
    match e {
        ssh::Error::Invalid(f, _) => ["alias", "host", "user", "port", "identity", "title", "description", "group", "color", "command"]
            .iter()
            .position(|x| x == f),
        ssh::Error::Duplicate(_) => Some(0),
        _ => None,
    }
}

fn interactive(p: &Paths, values: [String; 10], known: &[String]) -> std::io::Result<i32> {
    let spec = [
        (I18N.t("Alias"), I18N.t("name for `ssh <alias>`, for example build")),
        (Cow::Borrowed("HostName"), I18N.t("address or IP")),
        (Cow::Borrowed("User"), I18N.t("optional")),
        (Cow::Borrowed("Port"), Cow::Borrowed("22")),
        (Cow::Borrowed("IdentityFile"), I18N.t("~/.ssh/id_ed25519 (passwords are not stored)")),
        (I18N.t("Title"), I18N.t("in the saysdfTerm list")),
        (I18N.t("Description"), I18N.t("second column of the list")),
        (I18N.t("Group"), I18N.t("optional")),
        (I18N.t("Color"), I18N.t("red, blue, #rrggbb…")),
        (I18N.t("ssh command"), I18N.t("paste ssh -i \"~/.ssh/key\" user@host -p 22 — the fields will be filled in")),
    ];
    let mut fields: Vec<Field> = spec.iter().zip(values.iter()).map(|((l, h), v)| Field::new(l, h, v)).collect();
    let mut session = Session::enter()?;
    let mut saved: Option<(HostEntry, Vec<String>)> = None;
    let mut fatal: Option<String> = None;
    let ok = session.edit(&I18N.t("New SSH connection"), &mut fields, |fs| {
        for f in fs.iter_mut() {
            f.error = None;
        }
        let v: [String; 10] = std::array::from_fn(|i| fs[i].value.clone());
        // autofill: the ssh command and user@host are split into fields; the user checks and presses save again
        if !v[9].trim().is_empty() || ssh::split_user_host(&v[1]).is_some() {
            match build(&v) {
                Ok((e, _, notes)) => {
                    fs[1].value = e.hostname.clone();
                    fs[2].value = e.user.clone().unwrap_or_default();
                    fs[3].value = e.port.map(|x| x.to_string()).unwrap_or_default();
                    fs[4].value = e.identity.clone().unwrap_or_default();
                    fs[9].value.clear();
                    fs[1].error = Some(I18N.f(
                        "Fields filled in automatically ({notes}). Check them and save again.",
                        &[("notes", &notes.join("; "))],
                    ));
                }
                Err(e) => {
                    let i = field_of(&e).unwrap_or(9);
                    fs[i].error = Some(e.to_string());
                }
            }
            return false;
        }
        let res = build(&v).and_then(|(e, m, n)| save(p, &e, &m, known).map(|_| (e, n)));
        match res {
            Ok(e) => {
                saved = Some(e);
                true
            }
            Err(e) => match field_of(&e) {
                Some(i) => {
                    fs[i].error = Some(e.to_string());
                    false
                }
                None => {
                    fatal = Some(e.to_string());
                    true
                }
            },
        }
    })?;
    if !ok {
        return Ok(1);
    }
    if let Some(e) = fatal {
        session.message(&I18N.f("Write error: {error}", &[("error", &e)]))?;
        return Ok(1);
    }
    let Some((entry, _notes)) = saved else { return Ok(1) };
    let mut msg = I18N.f("Added Host {alias} to {path}", &[("alias", &entry.alias), ("path", &p.own.display())]);
    for n in entry.warnings() {
        msg = format!("{msg}\n{n}");
    }
    if !ssh::include_present(&p.config) {
        let q = format!(
            "{msg}\n\n{}",
            I18N.f(
                "Add the line \"{line}\" to the start of {path}?\nA backup will be made; the rest of the content is not changed.",
                &[("line", &ssh::INCLUDE_LINE), ("path", &p.config.display())]
            )
        );
        msg = if session.confirm(&q)? {
            format!("{msg}\n{}", include_msg(ssh::ensure_include(&p.config, ssh::INCLUDE_LINE)))
        } else {
            format!("{msg}\n{}", not_included(&entry.alias))
        };
    }
    session.message(&msg)?;
    Ok(0)
}
