use crate::termwindow::box_model::{ComputedElement, Element, ElementContent};
use crate::termwindow::saysdf_bars::{
    box_element, box_row, box_row_colors, colors, text_cells, truncate_cells, BarColors,
};
use crate::termwindow::{TermWindow, TermWindowNotif};
use ::window::{KeyCode, Modifiers, Window, WindowOps};
use mux::pane::{CachePolicy, Pane, PaneId};
use mux::Mux;
use parking_lot::{Condvar, Mutex};
use saysdf_hints::history::SessionHistory;
use saysdf_hints::suggest::{suggest, Context, Suggestion};
use saysdf_hints::Shell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

const DEBOUNCE: Duration = Duration::from_millis(30);
const ENABLE_VAR: &str = "SAYSDFTERM_SUGGEST";
const HISTFILE_VAR: &str = "SAYSDF_HISTFILE";
const PATH_VAR: &str = "SAYSDF_PATH";
const MAX_ROWS: usize = 5;

#[derive(Debug, Default)]
pub struct SuggestState {
    pub pane_id: Option<PaneId>,
    pub input: String,
    pub candidates: Vec<Suggestion>,
    pub visible: bool,
    pub selected: usize,
    pub armed: bool,
    pub drawn: bool,
    generation: u64,
    revision: u64,
}

#[derive(Debug)]
pub struct SuggestResult {
    generation: u64,
    pane_id: PaneId,
    input: String,
    candidates: Vec<Suggestion>,
}

impl SuggestState {
    fn begin(&mut self, pane_id: PaneId, input: String) -> u64 {
        self.clear();
        self.pane_id = Some(pane_id);
        self.input = input;
        self.generation
    }

    fn clear(&mut self) {
        self.generation += 1;
        self.revision += 1;
        self.pane_id = None;
        self.input.clear();
        self.candidates.clear();
        self.visible = false;
        self.drawn = false;
        self.selected = 0;
    }

    fn shown_in(&self, pane_id: PaneId, scrolled: bool) -> bool {
        self.visible && self.drawn && self.pane_id == Some(pane_id) && !scrolled
    }

    fn apply(
        &mut self,
        result: SuggestResult,
        current_input: impl FnOnce() -> Option<String>,
    ) -> bool {
        if result.generation != self.generation || self.pane_id != Some(result.pane_id) {
            return false;
        }
        if current_input().as_deref() != Some(result.input.as_str()) {
            return false;
        }
        self.candidates = result.candidates;
        self.revision += 1;
        self.visible = !self.candidates.is_empty();
        self.selected = self.selected.min(self.candidates.len().saturating_sub(1));
        true
    }

    fn on_key(&mut self, shown: bool, key: &KeyCode, mods: Modifiers) -> Action {
        let action = key_action(shown, key, mods);
        match action {
            Action::Pass => match arming(key, mods) {
                Arming::Arm => self.armed = true,
                Arming::Keep => {}
                Arming::Disarm => {
                    self.armed = false;
                    self.clear();
                }
            },
            Action::SelectNext => {
                let last = self.candidates.len().min(MAX_ROWS).saturating_sub(1);
                self.selected = (self.selected + 1).min(last);
            }
            Action::SelectPrev => self.selected = self.selected.saturating_sub(1),
            Action::Close => {
                self.armed = false;
                self.clear();
            }
            Action::Accept => {}
        }
        action
    }

    fn take_selected(&mut self) -> Option<String> {
        let insert = self.candidates.get(self.selected).map(|c| c.insert.clone());
        self.clear();
        insert
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Pass,
    Accept,
    SelectNext,
    SelectPrev,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arming {
    Arm,
    Keep,
    Disarm,
}

fn normalized(key: &KeyCode) -> KeyCode {
    match key {
        KeyCode::Physical(phys) => phys.to_key_code(),
        other => other.clone(),
    }
}

pub fn key_action(shown: bool, key: &KeyCode, mods: Modifiers) -> Action {
    if !shown || !mods.remove_positional_mods().is_empty() {
        return Action::Pass;
    }
    match normalized(key) {
        KeyCode::RightArrow | KeyCode::Char('\t') => Action::Accept,
        KeyCode::DownArrow => Action::SelectNext,
        KeyCode::UpArrow => Action::SelectPrev,
        KeyCode::Char('\u{1b}') => Action::Close,
        _ => Action::Pass,
    }
}

fn arming(key: &KeyCode, mods: Modifiers) -> Arming {
    let key = normalized(key);
    if key.is_modifier() {
        return Arming::Keep;
    }
    let mods = mods.remove_positional_mods();
    match key {
        KeyCode::Char('\u{08}') if mods.is_empty() => Arming::Keep,
        KeyCode::Char(c) if !c.is_control() && (mods - Modifiers::SHIFT).is_empty() => Arming::Arm,
        KeyCode::Composed(_) => Arming::Arm,
        _ => Arming::Disarm,
    }
}

struct Request {
    generation: u64,
    pane: Arc<dyn Pane>,
    input: String,
}

struct Slot<T> {
    next: Mutex<(Option<T>, bool)>,
    ready: Condvar,
}

impl<T> Slot<T> {
    fn new() -> Self {
        Slot {
            next: Mutex::new((None, false)),
            ready: Condvar::new(),
        }
    }

    fn put(&self, value: T) {
        self.next.lock().0 = Some(value);
        self.ready.notify_one();
    }

    fn take(&self) -> Option<T> {
        let mut next = self.next.lock();
        loop {
            if next.1 {
                return None;
            }
            if let Some(value) = next.0.take() {
                return Some(value);
            }
            self.ready.wait(&mut next);
        }
    }

    fn has_newer(&self) -> bool {
        let next = self.next.lock();
        next.0.is_some() || next.1
    }

    fn close(&self) {
        self.next.lock().1 = true;
        self.ready.notify_one();
    }
}

type Sessions = Arc<Mutex<HashMap<PaneId, SessionHistory>>>;

#[derive(Default)]
pub struct Suggest {
    pub state: SuggestState,
    pending: bool,
    sessions: Sessions,
    worker: Option<Arc<Slot<Request>>>,
    popup: Option<(PopupKey, ComputedElement)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PopupKey {
    revision: u64,
    selected: usize,
    pane: Rect,
    cursor: (usize, usize),
    cell: (isize, isize),
    dpi: usize,
    config: usize,
    atlas: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopupGeometry {
    pub rect: Rect,
    pub rows: usize,
}

pub fn popup_geometry(
    pane: Rect,
    cell: (f32, f32),
    cursor: (usize, usize),
    input_cells: usize,
    inner_cells: usize,
    candidates: usize,
) -> Option<PopupGeometry> {
    let (cell_w, cell_h) = cell;
    let wanted = candidates.min(MAX_ROWS);
    if wanted == 0 || cell_w <= 0. || cell_h <= 0. {
        return None;
    }
    let border = 2.;
    let cursor_top = pane.y + cursor.1 as f32 * cell_h;
    let cursor_bottom = cursor_top + cell_h;
    let fit = |space: f32| (((space - border) / cell_h).floor().max(0.) as usize).min(wanted);
    let below = fit(pane.y + pane.h - cursor_bottom);
    let above = fit(cursor_top - pane.y);
    let rows = if below == wanted || below >= above {
        below
    } else {
        above
    };
    if rows == 0 {
        return None;
    }
    let h = rows as f32 * cell_h + border;
    let y = if rows == below {
        cursor_bottom
    } else {
        cursor_top - h
    };
    let w = (inner_cells as f32 * cell_w + border).min(pane.w);
    let start = pane.x + cursor.0.saturating_sub(input_cells) as f32 * cell_w;
    let x = start.min(pane.x + pane.w - w).max(pane.x);
    Some(PopupGeometry {
        rect: Rect { x, y, w, h },
        rows,
    })
}

impl Drop for Suggest {
    fn drop(&mut self) {
        if let Some(slot) = &self.worker {
            slot.close();
        }
    }
}

fn submitted_command(
    keycode: &KeyCode,
    mods: Modifiers,
    input: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if *keycode != KeyCode::Char('\r') || !mods.remove_positional_mods().is_empty() {
        return None;
    }
    input().filter(|text| !text.is_empty())
}

fn record_command(
    sessions: &Sessions,
    pane_id: PaneId,
    command: &str,
    alive: impl Fn(PaneId) -> bool,
) {
    let mut sessions = sessions.lock();
    sessions.retain(|id, _| alive(*id));
    sessions.entry(pane_id).or_default().push(command);
}

fn local_shell(foreground: &str) -> Option<Shell> {
    let stem = Path::new(foreground)
        .file_stem()?
        .to_str()?
        .to_ascii_lowercase();
    match stem.as_str() {
        "bash" | "sh" => Some(Shell::Bash),
        "zsh" => Some(Shell::Zsh),
        "fish" => Some(Shell::Fish),
        "pwsh" | "powershell" => Some(Shell::Pwsh),
        _ => None,
    }
}

fn first_label(host: &str) -> &str {
    host.split('.').next().unwrap_or(host)
}

fn local_dir(url: &Url, local_host: Option<&str>) -> Option<PathBuf> {
    if url.scheme() != "file" {
        return None;
    }
    let is_local = match url.host_str() {
        None | Some("") | Some("localhost") => true,
        Some(host) => {
            local_host.is_some_and(|name| first_label(name).eq_ignore_ascii_case(first_label(host)))
        }
    };
    if !is_local {
        return None;
    }
    let mut url = url.clone();
    url.set_host(Some("localhost")).ok()?;
    url.to_file_path().ok()
}

fn compute(
    req: &Request,
    sessions: &Sessions,
    stale: &dyn Fn() -> bool,
    deliver: &dyn Fn(SuggestResult),
) {
    let pane_id = req.pane.pane_id();
    let session = sessions.lock().get(&pane_id).cloned().unwrap_or_default();
    let shell = mux_lua::saysdf_fg::foreground_process_name_cached(Arc::clone(&req.pane))
        .as_deref()
        .and_then(local_shell);
    let local_host = hostname::get().ok().and_then(|h| h.into_string().ok());
    let cwd = req
        .pane
        .get_current_working_dir(CachePolicy::AllowStale)
        .and_then(|url| local_dir(&url, local_host.as_deref()));

    let mut ctx = Context::new(shell.unwrap_or(Shell::Plain), &session);
    if let (Some(_), Some(cwd)) = (shell, cwd) {
        let vars = req.pane.copy_user_vars();
        ctx.local_sources = true;
        ctx.cwd = Some(cwd);
        ctx.history_file = vars.get(HISTFILE_VAR).map(PathBuf::from);
        ctx.path_var = vars
            .get(PATH_VAR)
            .cloned()
            .or_else(|| std::env::var("PATH").ok());
    }

    let full_pass = ctx.local_sources && ctx.sources.generators;
    ctx.sources.generators = false;
    let pass = |ctx: &Context, full: bool| {
        let candidates = suggest(&req.input, ctx);
        log::trace!(
            "saysdf_suggest: pane {} generation {} full {} local {} -> {} candidates",
            pane_id,
            req.generation,
            full,
            ctx.local_sources,
            candidates.len()
        );
        if stale() {
            return;
        }
        deliver(SuggestResult {
            generation: req.generation,
            pane_id,
            input: req.input.clone(),
            candidates,
        });
    };
    pass(&ctx, false);
    if full_pass && !stale() {
        ctx.sources.generators = true;
        pass(&ctx, true);
    }
}

fn run_worker<T>(slot: &Slot<T>, mut work: impl FnMut(T, &dyn Fn() -> bool)) {
    while let Some(req) = slot.take() {
        work(req, &|| slot.has_newer());
    }
}

fn spawn_worker(window: Window, sessions: Sessions) -> Option<Arc<Slot<Request>>> {
    let slot = Arc::new(Slot::new());
    let worker_slot = Arc::clone(&slot);
    let deliver = move |result: SuggestResult| {
        let window = window.clone();
        promise::spawn::spawn_into_main_thread(async move {
            window.notify(TermWindowNotif::Apply(Box::new(move |tw| {
                tw.saysdf_suggest_apply(result)
            })));
        })
        .detach();
    };
    match std::thread::Builder::new()
        .name("saysdf-suggest".into())
        .spawn(move || {
            run_worker(&worker_slot, |req, stale| {
                compute(&req, &sessions, stale, &deliver)
            })
        }) {
        Ok(_) => Some(slot),
        Err(err) => {
            log::error!("saysdf_suggest: std::thread::Builder::spawn: {err:#}; prompt suggestions are off for this window");
            None
        }
    }
}

impl TermWindow {
    fn saysdf_suggest_enabled(&self) -> bool {
        self.config
            .set_environment_variables
            .get(ENABLE_VAR)
            .is_some_and(|v| v == "1")
    }

    pub fn saysdf_suggest_key_down(
        &mut self,
        pane: &Arc<dyn Pane>,
        keycode: &KeyCode,
        mods: Modifiers,
        typed: bool,
    ) -> bool {
        if !self.saysdf_suggest_enabled() || self.get_modal().is_some() {
            return false;
        }
        let command = submitted_command(keycode, mods, || {
            pane.saysdf_prompt_input().map(|input| input.text)
        });
        if let Some(command) = command {
            let mux = Mux::get();
            record_command(
                &self.saysdf_suggest.sessions,
                pane.pane_id(),
                &command,
                |id| mux.get_pane(id).is_some(),
            );
        }
        if !typed {
            return false;
        }
        let pane_id = pane.pane_id();
        let scrolled = self.get_viewport(pane_id).is_some();
        let shown = self.saysdf_suggest.state.shown_in(pane_id, scrolled);
        let action = self.saysdf_suggest.state.on_key(shown, keycode, mods);
        let consumed = match action {
            Action::Pass => false,
            Action::Accept => self.saysdf_suggest_accept(pane),
            Action::SelectNext | Action::SelectPrev | Action::Close => true,
        };
        if consumed {
            if let Some(window) = &self.window {
                window.invalidate();
            }
        }
        consumed
    }

    fn saysdf_suggest_accept(&mut self, pane: &Arc<dyn Pane>) -> bool {
        let current = pane
            .saysdf_prompt_input()
            .filter(|input| input.cursor_at_end && input.text == self.saysdf_suggest.state.input);
        let insert = self.saysdf_suggest.state.take_selected();
        let (Some(_), Some(insert)) = (current, insert) else {
            self.saysdf_suggest.state.armed = false;
            return false;
        };
        if insert.chars().any(char::is_control) {
            log::error!(
                "saysdf_suggest: pane {}: candidate with control characters dropped, nothing inserted",
                pane.pane_id()
            );
            return true;
        }
        if let Err(err) = pane.send_paste(&insert) {
            log::error!(
                "saysdf_suggest: Pane::send_paste: pane {}: {err:#}",
                pane.pane_id()
            );
        }
        true
    }

    pub fn saysdf_suggest_output(&mut self, pane_id: PaneId) {
        if self.saysdf_suggest.pending || !self.saysdf_suggest_enabled() {
            return;
        }
        if self.get_active_pane_or_overlay().map(|p| p.pane_id()) != Some(pane_id) {
            return;
        }
        let Some(window) = self.window.clone() else {
            return;
        };
        self.saysdf_suggest.pending = true;
        promise::spawn::spawn(async move {
            smol::Timer::after(DEBOUNCE).await;
            window.notify(TermWindowNotif::Apply(Box::new(|tw| {
                tw.saysdf_suggest_fire()
            })));
        })
        .detach();
    }

    fn saysdf_suggest_fire(&mut self) {
        self.saysdf_suggest.pending = false;
        let Some(pane) = self.get_active_pane_or_overlay() else {
            return;
        };
        let pane_id = pane.pane_id();
        let input = pane
            .saysdf_prompt_input()
            .filter(|_| self.saysdf_suggest.state.armed)
            .filter(|input| input.cursor_at_end && !input.text.is_empty());
        let Some(input) = input else {
            self.saysdf_suggest.state.clear();
            return;
        };
        let state = &self.saysdf_suggest.state;
        if state.pane_id == Some(pane_id) && state.input == input.text {
            return;
        }
        if self.saysdf_suggest.worker.is_none() {
            let Some(window) = self.window.clone() else {
                return;
            };
            self.saysdf_suggest.worker =
                spawn_worker(window, Arc::clone(&self.saysdf_suggest.sessions));
        }
        let generation = self.saysdf_suggest.state.begin(pane_id, input.text.clone());
        if let Some(slot) = &self.saysdf_suggest.worker {
            slot.put(Request {
                generation,
                pane,
                input: input.text,
            });
        }
    }

    fn saysdf_suggest_apply(&mut self, result: SuggestResult) {
        let pane_id = result.pane_id;
        let applied = self.saysdf_suggest.state.apply(result, || {
            Mux::get()
                .get_pane(pane_id)
                .and_then(|pane| pane.saysdf_prompt_input())
                .map(|input| input.text)
        });
        if applied {
            if let Some(window) = &self.window {
                window.invalidate();
            }
        }
    }

    pub fn saysdf_paint_suggest(&mut self) -> anyhow::Result<()> {
        self.saysdf_suggest.state.drawn = false;
        if !self.saysdf_suggest.state.visible || self.get_modal().is_some() {
            return Ok(());
        }
        let Some(pos) = self.get_panes_to_render().into_iter().find(|p| p.is_active) else {
            return Ok(());
        };
        let pane_id = pos.pane.pane_id();
        if self.saysdf_suggest.state.pane_id != Some(pane_id)
            || self.get_viewport(pane_id).is_some()
        {
            return Ok(());
        }
        let cursor = pos.pane.get_cursor_position();
        let row = cursor.y - pos.pane.get_dimensions().physical_top;
        if row < 0 || row as usize >= pos.height {
            return Ok(());
        }
        let cell_w = self.render_metrics.cell_size.width as f32;
        let cell_h = self.render_metrics.cell_size.height as f32;
        let (padding_left, padding_top) = self.padding_left_top();
        let tab_bar_height = if self.show_tab_bar && !self.config.tab_bar_at_bottom {
            self.tab_bar_pixel_height()?
        } else {
            0.
        };
        let border = self.get_os_border();
        let pane = Rect {
            x: padding_left + border.left.get() as f32 + pos.left as f32 * cell_w,
            y: tab_bar_height + padding_top + border.top.get() as f32 + pos.top as f32 * cell_h,
            w: pos.width as f32 * cell_w,
            h: pos.height as f32 * cell_h,
        };
        let state = &self.saysdf_suggest.state;
        let key = PopupKey {
            revision: state.revision,
            selected: state.selected,
            pane,
            cursor: (cursor.x, row as usize),
            cell: (
                self.render_metrics.cell_size.width,
                self.render_metrics.cell_size.height,
            ),
            dpi: self.dimensions.dpi,
            config: self.config.generation(),
            atlas: self.saysdf_bars.atlas_epoch(),
        };
        if !matches!(&self.saysdf_suggest.popup, Some((k, _)) if *k == key) {
            self.saysdf_suggest.popup = None;
            let Some(computed) = self.saysdf_compute_suggest(pane, key)? else {
                return Ok(());
            };
            self.saysdf_suggest.popup = Some((key, computed));
        }
        if let Some((_, computed)) = &self.saysdf_suggest.popup {
            self.render_element(computed, self.render_state.as_ref().unwrap(), None)?;
            self.saysdf_suggest.state.drawn = true;
        }
        Ok(())
    }

    fn saysdf_compute_suggest(
        &self,
        pane: Rect,
        key: PopupKey,
    ) -> anyhow::Result<Option<ComputedElement>> {
        let state = &self.saysdf_suggest.state;
        let cell_w = self.render_metrics.cell_size.width.max(1) as f32;
        let cell_h = self.render_metrics.cell_size.height as f32;
        let shown = &state.candidates[..state.candidates.len().min(MAX_ROWS)];
        let max_cells = ((pane.w - 2.) / cell_w).floor().max(0.) as usize;
        let text_w = shown
            .iter()
            .map(|c| text_cells(&c.display))
            .max()
            .unwrap_or(0);
        let inner_cells = (text_w + 2).min(max_cells);
        let Some(geom) = popup_geometry(
            pane,
            (cell_w, cell_h),
            key.cursor,
            text_cells(&state.input),
            inner_cells,
            shown.len(),
        ) else {
            return Ok(None);
        };
        let font = self.fonts.default_font()?;
        let c = BarColors::from_config(&self.config);
        let inner_px = inner_cells as f32 * cell_w;
        let text_max = inner_cells.saturating_sub(2);
        let rows = shown
            .iter()
            .take(geom.rows)
            .enumerate()
            .map(|(idx, cand)| {
                let (bg, fg) = box_row_colors(&c, idx == state.selected);
                let text = truncate_cells(&cand.display, text_max);
                let (typed, rest) = match text.strip_prefix(state.input.as_str()) {
                    Some(rest) => (state.input.as_str(), rest),
                    None => ("", text.as_str()),
                };
                let parts = vec![
                    Element::new(&font, ElementContent::Text(format!(" {typed}")))
                        .colors(colors(bg, fg.mul_alpha(0.6))),
                    Element::new(&font, ElementContent::Text(format!("{rest} ")))
                        .colors(colors(bg, fg)),
                ];
                box_row(&font, parts, bg, fg, inner_px)
            })
            .collect();
        let element = box_element(&font, rows, &c, inner_px);
        let r = geom.rect;
        let computed = self.saysdf_compute_box(&element, euclid::rect(r.x, r.y, r.w, r.h), 100)?;
        Ok(Some(computed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use saysdf_hints::suggest::Source;

    fn cand(text: &str) -> Suggestion {
        Suggestion {
            display: text.into(),
            insert: text.into(),
            description: String::new(),
            source: Source::History,
        }
    }

    fn result(generation: u64, input: &str, text: &str) -> SuggestResult {
        SuggestResult {
            generation,
            pane_id: 1,
            input: input.into(),
            candidates: vec![cand(text)],
        }
    }

    const PANE: Rect = Rect {
        x: 10.,
        y: 20.,
        w: 800.,
        h: 400.,
    };
    const CELL: (f32, f32) = (10., 20.);

    #[test]
    fn popup_below_cursor_aligned_to_input_start() {
        let g = popup_geometry(PANE, CELL, (12, 3), 2, 20, 3).unwrap();
        assert_eq!(g.rows, 3);
        assert_eq!(g.rect.x, 10. + 10. * 10.);
        assert_eq!(g.rect.y, 20. + 4. * 20.);
        assert_eq!(g.rect.h, 3. * 20. + 2.);
        assert_eq!(g.rect.w, 20. * 10. + 2.);
    }

    #[test]
    fn popup_above_cursor_near_bottom() {
        let g = popup_geometry(PANE, CELL, (5, 18), 1, 20, 9).unwrap();
        assert_eq!(g.rows, 5);
        assert_eq!(g.rect.y + g.rect.h, 20. + 18. * 20.);
    }

    #[test]
    fn popup_shifted_left_and_clipped_at_right_edge() {
        let g = popup_geometry(PANE, CELL, (78, 0), 0, 20, 2).unwrap();
        assert_eq!(g.rect.x + g.rect.w, PANE.x + PANE.w);
        let wide = popup_geometry(PANE, CELL, (78, 0), 0, 200, 2).unwrap();
        assert_eq!(wide.rect.x, PANE.x);
        assert_eq!(wide.rect.w, PANE.w);
    }

    #[test]
    fn popup_rows_clipped_to_pane() {
        let small = Rect { h: 80., ..PANE };
        let g = popup_geometry(small, CELL, (0, 0), 0, 10, 5).unwrap();
        assert_eq!(g.rows, 2);
        assert!(g.rect.y + g.rect.h <= small.y + small.h);
        assert!(popup_geometry(PANE, CELL, (0, 0), 0, 10, 0).is_none());
    }

    #[test]
    fn latest_request_wins() {
        let slot = Slot::new();
        slot.put(1);
        slot.put(2);
        slot.put(3);
        assert!(slot.has_newer());
        assert_eq!(slot.take(), Some(3));
        assert!(!slot.has_newer());
        slot.close();
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn stale_result_dropped() {
        let mut state = SuggestState::default();
        let old = state.begin(1, "g".into());
        let new = state.begin(1, "gi".into());
        assert!(!state.apply(result(old, "g", "it"), || Some("gi".into())));
        assert!(state.candidates.is_empty());
        assert!(state.apply(result(new, "gi", "t"), || Some("gi".into())));
        assert_eq!(state.candidates, vec![cand("t")]);
        assert!(state.visible);
    }

    #[test]
    fn result_for_changed_input_discarded() {
        let mut state = SuggestState::default();
        let generation = state.begin(1, "gi".into());
        assert!(!state.apply(result(generation, "gi", "t"), || Some("gix".into())));
        assert!(!state.apply(result(generation, "gi", "t"), || None));
        assert!(state.candidates.is_empty());
        assert!(!state.visible);
    }

    #[test]
    fn full_pass_replaces_fast_pass() {
        let mut state = SuggestState::default();
        let generation = state.begin(1, "git c".into());
        assert!(state.apply(result(generation, "git c", "ommit"), || Some(
            "git c".into()
        )));
        assert!(state.apply(result(generation, "git c", "heckout"), || Some(
            "git c".into()
        )));
        assert_eq!(state.candidates, vec![cand("heckout")]);
    }

    #[test]
    fn cleared_state_drops_in_flight_result() {
        let mut state = SuggestState::default();
        let generation = state.begin(1, "ls".into());
        state.clear();
        assert!(!state.apply(result(generation, "ls", " -la"), || Some("ls".into())));
    }

    #[test]
    fn worker_takes_only_the_latest_and_sees_newer_requests() {
        let slot = Arc::new(Slot::new());
        slot.put(1);
        slot.put(2);
        let worker_slot = Arc::clone(&slot);
        let worker = std::thread::spawn(move || {
            let mut seen = vec![];
            run_worker(&worker_slot, |req, stale| {
                if req == 2 {
                    assert!(!stale());
                    worker_slot.put(3);
                    assert!(stale());
                }
                seen.push(req);
                if req == 3 {
                    worker_slot.close();
                }
            });
            seen
        });
        assert_eq!(worker.join().unwrap(), vec![2, 3]);
    }

    #[test]
    fn plain_enter_with_prompt_input_is_recorded() {
        let enter = KeyCode::Char('\r');
        let typed = || Some("make test".to_string());
        assert_eq!(
            submitted_command(&enter, Modifiers::NONE, typed),
            Some("make test".into())
        );
        assert_eq!(submitted_command(&enter, Modifiers::NONE, || None), None);
        assert_eq!(
            submitted_command(&enter, Modifiers::NONE, || Some(String::new())),
            None
        );
        assert_eq!(submitted_command(&enter, Modifiers::SHIFT, typed), None);
        assert_eq!(submitted_command(&enter, Modifiers::ALT, typed), None);
        assert_eq!(
            submitted_command(&KeyCode::Char('m'), Modifiers::NONE, typed),
            None
        );
    }

    #[test]
    fn session_history_grows_only_from_recorded_commands() {
        let sessions = Sessions::default();
        record_command(&sessions, 1, "make test", |_| true);
        record_command(&sessions, 2, "ls", |id| id != 1);
        let sessions = sessions.lock();
        assert!(!sessions.contains_key(&1));
        assert_eq!(sessions[&2].iter().collect::<Vec<_>>(), vec!["ls"]);
    }

    #[test]
    fn known_local_shells() {
        assert_eq!(local_shell("/usr/bin/bash"), Some(Shell::Bash));
        assert_eq!(local_shell("zsh"), Some(Shell::Zsh));
        assert_eq!(local_shell("pwsh.exe"), Some(Shell::Pwsh));
        assert_eq!(local_shell("powershell"), Some(Shell::Pwsh));
        assert_eq!(local_shell("ssh"), None);
        assert_eq!(local_shell("vim"), None);
    }

    #[test]
    fn remote_osc7_host_is_not_local() {
        // On Windows the url crate drops the host when the path starts with a
        // drive letter, so set the platform path after parsing.
        let (home, tmp) = if cfg!(windows) {
            ("/C:/Users/u", "/C:/Windows")
        } else {
            ("/home/u", "/tmp")
        };
        let mut url = Url::parse("file://build-box/x").unwrap();
        url.set_path(home);
        assert_eq!(url.host_str(), Some("build-box"));
        assert_eq!(local_dir(&url, Some("laptop")), None);
        assert!(local_dir(&url, Some("build-box.lan")).is_some());
        let mut url = Url::parse("file:///x").unwrap();
        url.set_path(tmp);
        assert!(local_dir(&url, None).is_some());
        let url = Url::parse("http://laptop/tmp").unwrap();
        assert_eq!(local_dir(&url, Some("laptop")), None);
    }
    fn shown_state(texts: &[&str]) -> SuggestState {
        let mut state = SuggestState::default();
        assert_eq!(
            state.on_key(false, &KeyCode::Char('g'), Modifiers::NONE),
            Action::Pass
        );
        assert!(state.armed);
        let generation = state.begin(1, "g".into());
        state.apply(
            SuggestResult {
                generation,
                pane_id: 1,
                input: "g".into(),
                candidates: texts.iter().map(|t| cand(t)).collect(),
            },
            || Some("g".into()),
        );
        assert!(state.visible);
        state.drawn = true;
        state
    }

    #[test]
    fn popup_not_drawn_passes_every_key() {
        let mut state = shown_state(&["it"]);
        assert!(state.shown_in(1, false));
        assert!(!state.shown_in(2, false));
        assert!(!state.shown_in(1, true));
        state.drawn = false;
        assert!(!state.shown_in(1, false));
        assert!(popup_geometry(Rect { h: 20., ..PANE }, CELL, (0, 0), 0, 10, 3).is_none());
        for key in &POPUP_KEYS {
            let shown = state.shown_in(1, false);
            assert_eq!(state.on_key(shown, key, Modifiers::NONE), Action::Pass);
        }
    }

    const POPUP_KEYS: [KeyCode; 5] = [
        KeyCode::RightArrow,
        KeyCode::Char('\t'),
        KeyCode::UpArrow,
        KeyCode::DownArrow,
        KeyCode::Char('\u{1b}'),
    ];

    #[test]
    fn popup_keys_act_only_while_shown_and_without_modifiers() {
        let none = Modifiers::NONE;
        assert_eq!(key_action(true, &KeyCode::RightArrow, none), Action::Accept);
        assert_eq!(key_action(true, &KeyCode::Char('\t'), none), Action::Accept);
        assert_eq!(
            key_action(true, &KeyCode::DownArrow, none),
            Action::SelectNext
        );
        assert_eq!(
            key_action(true, &KeyCode::UpArrow, none),
            Action::SelectPrev
        );
        assert_eq!(
            key_action(true, &KeyCode::Char('\u{1b}'), none),
            Action::Close
        );
        assert_eq!(
            key_action(true, &KeyCode::Physical(::window::PhysKeyCode::Tab), none),
            Action::Accept
        );
        assert_eq!(
            key_action(
                true,
                &KeyCode::Physical(::window::PhysKeyCode::Escape),
                none
            ),
            Action::Close
        );
        assert_eq!(
            key_action(true, &KeyCode::RightArrow, Modifiers::LEFT_SHIFT),
            Action::Accept
        );
        for key in &POPUP_KEYS {
            assert_eq!(key_action(false, key, none), Action::Pass);
            for mods in [
                Modifiers::SHIFT,
                Modifiers::CTRL,
                Modifiers::ALT,
                Modifiers::SUPER,
            ] {
                assert_eq!(key_action(true, key, mods), Action::Pass);
            }
        }
        for key in [
            KeyCode::Char('\r'),
            KeyCode::LeftArrow,
            KeyCode::Home,
            KeyCode::Char('a'),
            KeyCode::Char('\u{08}'),
        ] {
            assert_eq!(key_action(true, &key, none), Action::Pass);
            assert_eq!(key_action(false, &key, none), Action::Pass);
        }
    }

    #[test]
    fn printable_arms_backspace_keeps_other_keys_disarm() {
        assert_eq!(arming(&KeyCode::Char('a'), Modifiers::NONE), Arming::Arm);
        assert_eq!(arming(&KeyCode::Char('A'), Modifiers::SHIFT), Arming::Arm);
        assert_eq!(arming(&KeyCode::Char('ж'), Modifiers::NONE), Arming::Arm); // i18n-allow-cyrillic
        assert_eq!(
            arming(&KeyCode::Composed("й".into()), Modifiers::NONE), // i18n-allow-cyrillic
            Arming::Arm
        );
        assert_eq!(
            arming(&KeyCode::Char('\u{08}'), Modifiers::NONE),
            Arming::Keep
        );
        assert_eq!(arming(&KeyCode::LeftShift, Modifiers::SHIFT), Arming::Keep);
        assert_eq!(arming(&KeyCode::Char('a'), Modifiers::CTRL), Arming::Disarm);
        assert_eq!(arming(&KeyCode::Char('a'), Modifiers::ALT), Arming::Disarm);
        assert_eq!(
            arming(&KeyCode::Char('\u{08}'), Modifiers::CTRL),
            Arming::Disarm
        );
        for key in POPUP_KEYS.iter().chain(&[
            KeyCode::Char('\r'),
            KeyCode::LeftArrow,
            KeyCode::Home,
            KeyCode::Char('\u{7f}'),
        ]) {
            assert_eq!(arming(key, Modifiers::NONE), Arming::Disarm);
        }
    }

    #[test]
    fn hidden_popup_passes_everything_and_up_never_arms() {
        let mut state = SuggestState::default();
        for key in POPUP_KEYS.iter().chain(&[KeyCode::Char('\r')]) {
            assert_eq!(state.on_key(false, key, Modifiers::NONE), Action::Pass);
            assert!(!state.armed);
        }
    }

    #[test]
    fn selection_moves_without_wrap_and_resets() {
        let mut state = shown_state(&["it", "rep", "o", "x", "y", "z"]);
        let none = Modifiers::NONE;
        assert_eq!(
            state.on_key(true, &KeyCode::UpArrow, none),
            Action::SelectPrev
        );
        assert_eq!(state.selected, 0);
        for _ in 0..10 {
            state.on_key(true, &KeyCode::DownArrow, none);
        }
        assert_eq!(state.selected, MAX_ROWS - 1);
        state.on_key(true, &KeyCode::UpArrow, none);
        assert_eq!(state.selected, MAX_ROWS - 2);
        assert_eq!(
            state.on_key(true, &KeyCode::Char('\t'), none),
            Action::Accept
        );
        assert_eq!(state.take_selected(), Some("x".into()));
        assert_eq!(state.selected, 0);
        assert!(!state.visible);
        assert!(state.armed);

        let mut state = shown_state(&["it", "rep"]);
        state.on_key(true, &KeyCode::DownArrow, none);
        assert_eq!(state.selected, 1);
        state.on_key(false, &KeyCode::Char('i'), none);
        let generation = state.begin(1, "gi".into());
        state.apply(result(generation, "gi", "t"), || Some("gi".into()));
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn esc_closes_and_only_a_printable_rearms() {
        let none = Modifiers::NONE;
        let mut state = shown_state(&["it"]);
        assert_eq!(
            state.on_key(true, &KeyCode::Char('\u{1b}'), none),
            Action::Close
        );
        assert!(!state.visible && !state.armed);
        assert_eq!(state.on_key(false, &KeyCode::UpArrow, none), Action::Pass);
        assert!(!state.armed);
        state.on_key(false, &KeyCode::Char('\u{08}'), none);
        assert!(!state.armed);
        state.on_key(false, &KeyCode::Char('x'), none);
        assert!(state.armed);
    }

    #[test]
    fn backspace_keeps_open_other_keys_close() {
        let none = Modifiers::NONE;
        let mut state = shown_state(&["it"]);
        assert_eq!(
            state.on_key(true, &KeyCode::Char('\u{08}'), none),
            Action::Pass
        );
        assert!(state.visible && state.armed);
        assert_eq!(
            state.on_key(true, &KeyCode::LeftShift, Modifiers::SHIFT),
            Action::Pass
        );
        assert!(state.visible && state.armed);
        for key in [KeyCode::Char('\r'), KeyCode::LeftArrow, KeyCode::Char('\t')] {
            let mut state = shown_state(&["it"]);
            let mods = if key == KeyCode::Char('\t') {
                Modifiers::SHIFT
            } else {
                none
            };
            assert_eq!(state.on_key(true, &key, mods), Action::Pass);
            assert!(!state.visible && !state.armed);
        }
    }
}
