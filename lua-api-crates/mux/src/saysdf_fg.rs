// saysdfTerminal: fork file. Foreground process name for Lua without blocking the GUI thread.
//
// Upstream `pane:get_foreground_process_name()` always uses `CachePolicy::FetchImmediate`,
// and on Windows that is a process tree walk (~10 ms) right inside the `update-status` handler.
// `CachePolicy::AllowStale` does not help: the `LocalPane` cache TTL is 300 ms while the status
// tick is 1 s, so every call walks the tree synchronously again.
//
// Here: stale-while-revalidate per pane_id. A call returns the last known answer at once and, if
// it is older than `REFRESH_AFTER`, starts a refresh on a separate thread (one per pane at a time).
// Only the very first request for a pane is synchronous. The answer lags reality by at most one
// call (a status tick), which is enough for hints/context.
use mux::pane::{CachePolicy, Pane, PaneId};
use mux::Mux;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

const REFRESH_AFTER: Duration = Duration::from_millis(300);

struct Entry {
    name: Option<String>,
    at: Instant,
    refreshing: bool,
}

static CACHE: LazyLock<Mutex<HashMap<PaneId, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn refresh_in_background(pane: Arc<dyn Pane>) {
    let pane_id = pane.pane_id();
    let spawned = std::thread::Builder::new()
        .name("saysdf-fg-process".into())
        .spawn(move || {
            let name = pane.get_foreground_process_name(CachePolicy::AllowStale);
            drop(pane);
            let alive = Mux::try_get()
                .map(|mux| mux.get_pane(pane_id).is_some())
                .unwrap_or(false);
            let mut cache = CACHE.lock();
            if alive {
                cache.insert(
                    pane_id,
                    Entry {
                        name,
                        at: Instant::now(),
                        refreshing: false,
                    },
                );
            } else {
                cache.remove(&pane_id);
            }
        });
    if spawned.is_err() {
        if let Some(e) = CACHE.lock().get_mut(&pane_id) {
            e.refreshing = false;
        }
    }
}

pub fn foreground_process_name_cached(pane: Arc<dyn Pane>) -> Option<String> {
    let pane_id = pane.pane_id();
    {
        let mut cache = CACHE.lock();
        if let Some(e) = cache.get_mut(&pane_id) {
            let name = e.name.clone();
            if !e.refreshing && e.at.elapsed() >= REFRESH_AFTER {
                e.refreshing = true;
                drop(cache);
                refresh_in_background(pane);
            }
            return name;
        }
    }
    let name = pane.get_foreground_process_name(CachePolicy::AllowStale);
    CACHE.lock().insert(
        pane_id,
        Entry {
            name: name.clone(),
            at: Instant::now(),
            refreshing: false,
        },
    );
    name
}
