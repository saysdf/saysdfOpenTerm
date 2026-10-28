//! Worker thread owning the connection (design D3): the UI is not blocked by a long query,
//! cancellation goes through the driver's `CancelHandle`.

use saysdf_db::export::{self, Format};
use saysdf_db::{
    Access, Backend, CancelHandle, ConnectionConfig, DoltBranch, DoltCommit, PageRequest,
    ResultSet, TableDesc, TableInfo, TableRef,
};
use saysdf_secrets::{Domain, Secrets};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-db");

pub enum Req {
    Schemas,
    Tables(Option<String>),
    Describe(TableRef),
    Page(PageRequest),
    Query(String, usize),
    /// Dolt: list of branches.
    Branches,
    /// Dolt: history of the current branch or of a ref (`AS OF`).
    Log(Option<String>),
    /// The whole table (sort/filter/AS OF from the page request) to a file, streamed.
    ExportTable(PageRequest, PathBuf),
    /// All rows of a query to a file; the query is re-run read-only.
    ExportQuery(String, PathBuf),
}

const LOG_LIMIT: u32 = 200;

/// Writes the export to `path`; on error the partial file is removed.
fn export_to(
    path: &Path,
    f: impl FnOnce(std::io::BufWriter<std::fs::File>, Format) -> saysdf_db::Result<u64>,
) -> Result<u64, String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let r = f(std::io::BufWriter::new(file), Format::from_path(path)).map_err(|e| e.to_string());
    if r.is_err() {
        let _ = std::fs::remove_file(path);
    }
    r
}

pub enum Resp {
    Opened(Result<Option<Box<dyn CancelHandle>>, String>),
    Schemas(Result<Vec<String>, String>),
    Tables(Option<String>, Result<Vec<TableInfo>, String>),
    Describe(TableRef, Result<TableDesc, String>),
    Page(PageRequest, Result<ResultSet, String>),
    Query(Result<ResultSet, String>, Duration),
    Branches(Result<Vec<DoltBranch>, String>),
    Log(Option<String>, Result<Vec<DoltCommit>, String>),
    Exported(PathBuf, Result<u64, String>),
}

pub struct Worker {
    tx: Sender<Req>,
    rx: Receiver<Resp>,
    cancel: Option<Box<dyn CancelHandle>>,
    pub busy: usize,
    pub write: bool,
}

/// The password from the secret store, if the database is a network one and has one there.
pub fn stored_password(cfg: &ConnectionConfig) -> Option<String> {
    if !cfg.kind.needs_password() {
        return None;
    }
    Secrets::os().get(Domain::Db, &cfg.id).ok()
}

impl Worker {
    pub fn spawn(cfg: ConnectionConfig, password: Option<String>, access: Access) -> Self {
        let (tx, req_rx) = channel::<Req>();
        let (resp_tx, rx) = channel::<Resp>();
        let write = !access.is_read_only();
        std::thread::spawn(move || {
            let mut be: Box<dyn Backend> = match saysdf_db::open(&cfg, password.as_deref(), access)
            {
                Ok(b) => b,
                Err(e) => {
                    let _ = resp_tx.send(Resp::Opened(Err(e.to_string())));
                    return;
                }
            };
            drop(password);
            if resp_tx.send(Resp::Opened(Ok(be.cancel_handle()))).is_err() {
                return;
            }
            let s = |e: saysdf_db::Error| e.to_string();
            while let Ok(req) = req_rx.recv() {
                let resp = match req {
                    Req::Schemas => Resp::Schemas(be.list_schemas().map_err(s)),
                    Req::Tables(sc) => {
                        let r = be.list_tables(sc.as_deref()).map_err(s);
                        Resp::Tables(sc, r)
                    }
                    Req::Describe(t) => {
                        let r = be.describe(&t).map_err(s);
                        Resp::Describe(t, r)
                    }
                    Req::Page(p) => {
                        let r = be.fetch_page(&p).map_err(s);
                        Resp::Page(p, r)
                    }
                    Req::Query(sql, max) => {
                        let t = Instant::now();
                        let r = be.query(&sql, max).map_err(s);
                        Resp::Query(r, t.elapsed())
                    }
                    Req::Branches => Resp::Branches(be.dolt_branches().map_err(s)),
                    Req::Log(at) => {
                        let r = be.dolt_log(LOG_LIMIT, at.as_deref()).map_err(s);
                        Resp::Log(at, r)
                    }
                    Req::ExportTable(p, path) => {
                        let r = export_to(&path, |w, fmt| {
                            export::export_table(be.as_mut(), &p, fmt, w)
                        });
                        Resp::Exported(path, r)
                    }
                    Req::ExportQuery(sql, path) => {
                        let r = export_to(&path, |w, fmt| {
                            export::export_query(be.as_mut(), &sql, fmt, w)
                        });
                        Resp::Exported(path, r)
                    }
                };
                if resp_tx.send(resp).is_err() {
                    return;
                }
            }
        });
        Self {
            tx,
            rx,
            cancel: None,
            busy: 1,
            write,
        }
    }

    pub fn send(&mut self, r: Req) {
        if self.tx.send(r).is_ok() {
            self.busy += 1;
        }
    }

    /// The next response, if ready. `Err(())`: the thread has finished.
    pub fn poll(&mut self) -> Result<Option<Resp>, ()> {
        match self.rx.try_recv() {
            Ok(r) => {
                self.busy = self.busy.saturating_sub(1);
                Ok(Some(r))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(()),
        }
    }

    pub fn set_cancel(&mut self, c: Option<Box<dyn CancelHandle>>) {
        self.cancel = c;
    }

    pub fn cancel(&self) -> Result<(), String> {
        match &self.cancel {
            Some(c) => c.cancel().map_err(|e| e.to_string()),
            None => Err(I18N.t("cancelling is not supported by this driver").into_owned()),
        }
    }
}
