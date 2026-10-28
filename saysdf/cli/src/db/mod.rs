//! `saysdf db`: database viewer (change database-viewer, design D1-D9). Read-only by default;
//! writes only through "DANGEROUS MODE", enabled by the toolbar button and a confirmation that
//! names the database.

mod conns;
mod state;
mod ui;
mod worker;

use clap::{Args, Subcommand};
use saysdf_db::{Access, ConnectionConfig};
use std::path::PathBuf;

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-db");

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct DbArgs {
    #[command(subcommand)]
    pub cmd: Option<DbCmd>,
    /// SQLite file to open right away (read-only)
    pub path: Option<PathBuf>,
    /// SQLite file (same as the positional path)
    #[arg(long, value_name = "FILE", conflicts_with = "path")]
    pub sqlite: Option<PathBuf>,
    /// Open a saved connection by id
    #[arg(long, visible_alias = "conn", value_name = "ID")]
    pub connection: Option<String>,
    /// Extra connections file (TOML, `[[connection]]`), read-only; the main one is
    /// `<config_dir>/saysdf/db-connections.toml`
    #[arg(long, value_name = "FILE")]
    pub connections: Option<PathBuf>,
    /// Run a query (read-only); with `--print`, without the interface
    #[arg(long, value_name = "SQL")]
    pub query: Option<String>,
    /// Print the `--query` result to stdout and exit (for debugging and scripts)
    #[arg(long)]
    pub print: bool,
    /// With `--print`: list the tables of a schema (empty: the current schema) instead of a query
    #[arg(long, value_name = "SCHEMA", num_args = 0..=1, default_missing_value = "", conflicts_with = "query")]
    pub tables: Option<String>,
    /// Format of `--print`: csv or json
    #[arg(long, default_value = "csv", value_parser = ["csv", "json"])]
    pub format: String,
    /// Row limit of a query result
    #[arg(long, default_value_t = 10_000)]
    pub max_rows: usize,
    /// Save the query history to a file (by default it stays in memory; dangerous-mode queries
    /// are never written to disk)
    #[arg(long, value_name = "FILE")]
    pub save_history: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum DbCmd {
    /// Add a "PostgreSQL over SSH" connection: `ssh <alias> [docker exec -i <container>]
    /// psql`, with no open ports or tunnels; no password needed (socket on the server)
    AddSsh(AddSshArgs),
}

#[derive(Args)]
pub struct AddSshArgs {
    /// Connection id (Latin letters, digits, - _ .)
    #[arg(long)]
    pub name: String,
    /// Host from ~/.ssh/config (key login, BatchMode)
    #[arg(long, value_name = "ALIAS")]
    pub ssh: String,
    /// Docker container with psql; without it psql runs directly on the host
    #[arg(long)]
    pub container: Option<String>,
    /// Database
    #[arg(long)]
    pub db: String,
    /// PostgreSQL role (preferably SELECT only)
    #[arg(long)]
    pub user: String,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    /// Connections file (default: the main `<config_dir>/saysdf/db-connections.toml`)
    #[arg(long, value_name = "FILE")]
    pub file: Option<PathBuf>,
}

fn add_ssh(a: AddSshArgs) -> i32 {
    if let Err(e) = conns::validate_id(&a.name) {
        eprintln!("saysdf db add-ssh: --name: {e}");
        return 2;
    }
    let mut c = ConnectionConfig::pg_ssh(&a.name, &a.ssh, a.container.as_deref(), &a.db, &a.user);
    c.title = a.title;
    c.description = a.description;
    if let Err(e) = saysdf_db::pg_ssh::SshTarget::from_config(&c) {
        eprintln!("saysdf db add-ssh: {e}");
        return 2;
    }
    let Some(path) = a.file.or_else(conns::default_path) else {
        eprintln!("saysdf db add-ssh: {}", I18N.t("config directory not found"));
        return 2;
    };
    let mut list = match conns::load(&path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("saysdf db add-ssh: {e}");
            return 2;
        }
    };
    conns::upsert(&mut list, c);
    if let Err(e) = conns::save(&path, &list) {
        eprintln!("saysdf db add-ssh: {e}");
        return 1;
    }
    println!("{}: {}", path.display(), a.name);
    0
}

pub fn run(args: DbArgs) -> i32 {
    if let Some(DbCmd::AddSsh(a)) = args.cmd {
        return add_ssh(a);
    }
    let main_path = conns::default_path();
    let mut list = match main_path.as_deref().map(conns::load).transpose() {
        Ok(v) => v.unwrap_or_default(),
        Err(e) => {
            eprintln!("saysdf db: {e}");
            return 2;
        }
    };
    let saved_ids: Vec<String> = list.iter().map(|c| c.id.clone()).collect();
    if let Some(extra) = &args.connections {
        match conns::load(extra) {
            Ok(v) => {
                for c in v {
                    if !list.iter().any(|x| x.id == c.id) {
                        list.push(c);
                    }
                }
            }
            Err(e) => {
                eprintln!("saysdf db: {e}");
                return 2;
            }
        }
    }
    let file = args.sqlite.clone().or(args.path.clone());
    let initial: Option<ConnectionConfig> = if let Some(f) = &file {
        Some(ConnectionConfig::sqlite("file", &f.to_string_lossy()))
    } else if let Some(id) = &args.connection {
        match list.iter().find(|c| &c.id == id) {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("saysdf db: {}", I18N.f("connection “{id}” not found", &[("id", id)]));
                return 2;
            }
        }
    } else {
        None
    };

    if args.print {
        let Some(cfg) = initial else {
            eprintln!("saysdf db: {}", I18N.t("--print needs --sqlite <file>, a path or --connection <id>"));
            return 2;
        };
        if let Some(schema) = &args.tables {
            return print_tables(&cfg, schema, &args.format);
        }
        let Some(sql) = args.query.as_deref() else {
            eprintln!("saysdf db: {}", I18N.t("--print needs --query or --tables"));
            return 2;
        };
        return print_query(&cfg, sql, &args.format, args.max_rows);
    }

    let opts = ui::Options {
        main_path,
        saved_ids,
        initial,
        query: args.query,
        max_rows: args.max_rows,
        save_history: args.save_history,
    };
    match ui::run(list, opts) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("saysdf db: {e}");
            1
        }
    }
}

/// List of tables (driver introspection) shaped like a query result: schema, name, kind.
fn print_tables(cfg: &ConnectionConfig, schema: &str, format: &str) -> i32 {
    print_result(cfg, format, usize::MAX, |be| {
        let schema = (!schema.is_empty()).then_some(schema);
        let tables = be.list_tables(schema)?;
        Ok(saysdf_db::ResultSet {
            columns: ["schema", "name", "kind"].map(String::from).to_vec(),
            rows: tables
                .into_iter()
                .map(|t| {
                    let kind = match t.kind {
                        saysdf_db::ObjectKind::Table => "table",
                        saysdf_db::ObjectKind::View => "view",
                    };
                    vec![
                        saysdf_db::Value::Text(t.schema.unwrap_or_default()),
                        saysdf_db::Value::Text(t.name),
                        saysdf_db::Value::Text(kind.into()),
                    ]
                })
                .collect(),
            ..Default::default()
        })
    })
}

/// Non-interactive query: always read-only; the password comes only from the secret store.
fn print_query(cfg: &ConnectionConfig, sql: &str, format: &str, max_rows: usize) -> i32 {
    print_result(cfg, format, max_rows, |be| be.query(sql, max_rows))
}

fn print_result(
    cfg: &ConnectionConfig,
    format: &str,
    max_rows: usize,
    f: impl FnOnce(&mut dyn saysdf_db::Backend) -> saysdf_db::Result<saysdf_db::ResultSet>,
) -> i32 {
    let password = worker::stored_password(cfg);
    let mut be = match saysdf_db::open(cfg, password.as_deref(), Access::ReadOnly) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("saysdf db: {e}");
            return 1;
        }
    };
    let rs = match f(be.as_mut()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("saysdf db: {e}");
            return 1;
        }
    };
    let out = std::io::stdout();
    let mut lock = out.lock();
    let r = if format == "json" {
        saysdf_db::export::to_json(&rs, &mut lock)
    } else {
        saysdf_db::export::to_csv(&rs, &mut lock)
    };
    if let Err(e) = r {
        eprintln!("saysdf db: {e}");
        return 1;
    }
    if rs.truncated {
        eprintln!("saysdf db: {}", I18N.f("result truncated to {max} rows", &[("max", &max_rows)]));
    }
    0
}
