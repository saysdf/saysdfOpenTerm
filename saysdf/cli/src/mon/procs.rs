//! Process list: collection, filter, sorting, tree, the confirmation decision
//! (system-monitor D2, D3). Platform actions are in `actions`.
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

static I18N: saysdf_i18n::Domain = saysdf_i18n::Domain::new("cli-mon");

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcRow {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub name: String,
    pub cmd: String,
    pub exe: Option<std::path::PathBuf>,
    pub cpu: f32,
    pub mem: u64,
    /// uid (Unix) / SID (Windows) as a string; None means the OS did not provide it (usually a system or foreign process).
    pub user: Option<String>,
    pub start: u64,
}

pub fn refresh(sys: &mut System) {
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_user(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet)
            .with_exe(UpdateKind::OnlyIfNotSet),
    );
}

pub fn collect(sys: &System) -> Vec<ProcRow> {
    sys.processes()
        .values()
        // Linux threads (tasks) are not shown as separate rows
        .filter(|p| p.thread_kind().is_none_or(|k| k != sysinfo::ThreadKind::Userland))
        .map(|p| ProcRow {
            pid: p.pid().as_u32(),
            ppid: p.parent().map(|x| x.as_u32()),
            name: p.name().to_string_lossy().into_owned(),
            cmd: p
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" "),
            exe: p.exe().map(|e| e.to_path_buf()),
            cpu: p.cpu_usage(),
            mem: p.memory(),
            user: p.user_id().map(|u| u.to_string()),
            start: p.start_time(),
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Cpu,
    Mem,
    Pid,
    Name,
}

impl SortKey {
    pub fn next(self) -> Self {
        match self {
            SortKey::Cpu => SortKey::Mem,
            SortKey::Mem => SortKey::Pid,
            SortKey::Pid => SortKey::Name,
            SortKey::Name => SortKey::Cpu,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Cpu => "CPU",
            SortKey::Mem => "MEM",
            SortKey::Pid => "PID",
            SortKey::Name => "NAME",
        }
    }
}

/// Sorting: CPU and memory descending, PID and name ascending.
pub fn sort_rows(rows: &mut [ProcRow], key: SortKey) {
    match key {
        SortKey::Cpu => rows.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(a.pid.cmp(&b.pid))),
        SortKey::Mem => rows.sort_by(|a, b| b.mem.cmp(&a.mem).then(a.pid.cmp(&b.pid))),
        SortKey::Pid => rows.sort_by_key(|r| r.pid),
        SortKey::Name => rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.pid.cmp(&b.pid))),
    }
}

/// Case-insensitive filter by name, command line and PID.
pub fn matches(r: &ProcRow, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || r.name.to_lowercase().contains(&q)
        || r.cmd.to_lowercase().contains(&q)
        || r.pid.to_string() == q
}

/// Tree: traversal order `(index in rows, depth)`. Orphans (parent not in the list) are roots;
/// PPID cycles are broken (every process is output exactly once). Children follow the order of `rows`.
pub fn build_tree(rows: &[ProcRow]) -> Vec<(usize, usize)> {
    let pids: HashMap<u32, usize> = rows.iter().enumerate().map(|(i, r)| (r.pid, i)).collect();
    let mut children: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut roots = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        match r.ppid {
            Some(pp) if pp != r.pid && pids.contains_key(&pp) => children.entry(pp).or_default().push(i),
            _ => roots.push(i),
        }
    }
    let mut out = Vec::with_capacity(rows.len());
    let mut seen = vec![false; rows.len()];
    let walk = |start: usize, out: &mut Vec<(usize, usize)>, seen: &mut Vec<bool>| {
        let mut stack = vec![(start, 0usize)];
        while let Some((i, d)) = stack.pop() {
            if seen[i] {
                continue;
            }
            seen[i] = true;
            out.push((i, d));
            if let Some(ch) = children.get(&rows[i].pid) {
                for &c in ch.iter().rev() {
                    if !seen[c] {
                        stack.push((c, d + 1));
                    }
                }
            }
        }
    };
    for &r in &roots {
        walk(r, &mut out, &mut seen);
    }
    // what is left: cycles without a root
    for i in 0..rows.len() {
        if !seen[i] {
            walk(i, &mut out, &mut seen);
        }
    }
    out
}

/// The PID and all of its descendants.
pub fn descendants(rows: &[ProcRow], pid: u32) -> HashSet<u32> {
    let mut set = HashSet::from([pid]);
    loop {
        let before = set.len();
        for r in rows {
            if let Some(pp) = r.ppid {
                if set.contains(&pp) && r.pid != pp {
                    set.insert(r.pid);
                }
            }
        }
        if set.len() == before {
            return set;
        }
    }
}

/// Ancestors of the PID (not including itself), guarded against cycles.
pub fn ancestors(rows: &[ProcRow], pid: u32) -> HashSet<u32> {
    let parent: HashMap<u32, Option<u32>> = rows.iter().map(|r| (r.pid, r.ppid)).collect();
    let mut set = HashSet::new();
    let mut cur = pid;
    while let Some(Some(pp)) = parent.get(&cur) {
        if *pp == cur || !set.insert(*pp) {
            break;
        }
        cur = *pp;
    }
    set
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Suspend,
    Resume,
    Stop,
    Kill,
    PriorityLower,
    PriorityRaise,
    Tree,
    CopyCmd,
    OpenDir,
}

impl Action {
    pub const MENU: [Action; 9] = [
        Action::Suspend,
        Action::Resume,
        Action::Stop,
        Action::Kill,
        Action::PriorityLower,
        Action::PriorityRaise,
        Action::Tree,
        Action::CopyCmd,
        Action::OpenDir,
    ];
    /// Context menu item text (max ~34 characters including the number).
    pub fn label(self) -> Cow<'static, str> {
        match self {
            Action::Suspend => I18N.t("Pause"),
            Action::Resume => I18N.t("Resume"),
            Action::Stop => I18N.t("Soft stop"),
            Action::Kill => Cow::Borrowed("Kill"),
            Action::PriorityLower => I18N.t("Lower priority"),
            Action::PriorityRaise => I18N.t("Raise priority"),
            Action::Tree => I18N.t("Show descendants"),
            Action::CopyCmd => I18N.t("Copy command line"),
            Action::OpenDir => I18N.t("Open exe folder"),
        }
    }
    /// The action changes the process state in a way where a mistake is costly.
    pub fn dangerous(self) -> bool {
        matches!(self, Action::Suspend | Action::Stop | Action::Kill)
    }
}

/// Who is acting: the owner (uid/SID) and the PIDs that must not be touched without asking
/// (the monitor itself and its ancestors: the terminal, the tab shell).
#[derive(Clone, Debug, Default)]
pub struct Me {
    pub user: Option<String>,
    pub protected: HashSet<u32>,
}

/// Whether confirmation is needed (design D3): only for dangerous actions and when the process
/// is a system one (PID <= 4), foreign or of unknown owner, or the terminal/an ancestor of the monitor.
pub fn needs_confirm(target: &ProcRow, action: Action, me: &Me) -> bool {
    if !action.dangerous() {
        return false;
    }
    if target.pid <= 4 || me.protected.contains(&target.pid) {
        return true;
    }
    match (&target.user, &me.user) {
        (Some(t), Some(m)) => t != m,
        _ => true,
    }
}

/// Confirmation prompt text (spec: `Terminate <name> (PID n) owned by <user>? Press Y to confirm…`).
pub fn confirm_text(target: &ProcRow, action: Action) -> String {
    let who = [
        ("name", &target.name as &dyn std::fmt::Display),
        ("pid", &target.pid),
        ("user", &target.user.as_deref().unwrap_or("?")),
        ("hint", &crate::keys::CONFIRM_HINT),
    ];
    // only dangerous actions (suspend, stop, kill) ask for confirmation
    match action {
        Action::Suspend => I18N.f("Suspend {name} (PID {pid}) owned by {user}? {hint}", &who),
        _ => I18N.f("Terminate {name} (PID {pid}) owned by {user}? {hint}", &who),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: Option<u32>, name: &str) -> ProcRow {
        ProcRow { pid, ppid, name: name.into(), user: Some("me".into()), ..Default::default() }
    }

    #[test]
    fn filter_and_sort() {
        let mut rows = vec![
            ProcRow { mem: 10, cmd: "cargo build".into(), ..p(10, None, "cargo") },
            ProcRow { mem: 50, ..p(11, None, "rustc") },
            ProcRow { mem: 30, cmd: "x --cargo".into(), ..p(12, None, "sh") },
        ];
        rows.retain(|r| matches(r, "CARGO"));
        sort_rows(&mut rows, SortKey::Mem);
        assert_eq!(rows.iter().map(|r| r.pid).collect::<Vec<_>>(), vec![12, 10]);
        assert!(matches(&p(77, None, "a"), "77"));
    }

    #[test]
    fn tree_orphans_and_cycles() {
        let rows = vec![
            p(1, None, "init"),
            p(2, Some(1), "a"),
            p(3, Some(2), "b"),
            p(4, Some(999), "orphan"),
            p(5, Some(6), "c1"), // cycle 5 <-> 6
            p(6, Some(5), "c2"),
            p(7, Some(7), "self"),
        ];
        let t = build_tree(&rows);
        assert_eq!(t.len(), rows.len());
        let order: Vec<(u32, usize)> = t.iter().map(|&(i, d)| (rows[i].pid, d)).collect();
        assert_eq!(&order[..3], &[(1, 0), (2, 1), (3, 2)]);
        assert!(order.contains(&(4, 0)));
        assert!(order.contains(&(7, 0)));
        let mut pids: Vec<u32> = order.iter().map(|x| x.0).collect();
        pids.sort();
        pids.dedup();
        assert_eq!(pids.len(), rows.len());
    }

    #[test]
    fn descendants_and_ancestors() {
        let rows = vec![p(1, None, "i"), p(2, Some(1), "a"), p(3, Some(2), "b"), p(4, Some(1), "c"), p(5, Some(6), "x"), p(6, Some(5), "y")];
        assert_eq!(descendants(&rows, 2), HashSet::from([2, 3]));
        assert_eq!(ancestors(&rows, 3), HashSet::from([1, 2]));
        assert_eq!(ancestors(&rows, 5), HashSet::from([6, 5]));
    }

    #[test]
    fn confirm_rules() {
        let me = Me { user: Some("me".into()), protected: HashSet::from([100, 101]) };
        let own = p(500, None, "own");
        assert!(!needs_confirm(&own, Action::Kill, &me));
        let foreign = ProcRow { user: Some("root".into()), ..p(501, None, "f") };
        assert!(needs_confirm(&foreign, Action::Kill, &me));
        assert!(!needs_confirm(&foreign, Action::CopyCmd, &me));
        assert!(!needs_confirm(&foreign, Action::Resume, &me));
        let unknown = ProcRow { user: None, ..p(502, None, "u") };
        assert!(needs_confirm(&unknown, Action::Stop, &me));
        assert!(needs_confirm(&p(4, None, "System"), Action::Suspend, &me));
        // the terminal / an ancestor
        assert!(needs_confirm(&p(101, None, "saysdfTerm"), Action::Kill, &me));
        assert_eq!(
            confirm_text(&foreign, Action::Kill),
            format!("Terminate f (PID 501) owned by root? {}", crate::keys::CONFIRM_HINT)
        );
    }

    #[test]
    fn confirm_and_labels_follow_language() {
        let foreign = ProcRow { user: Some("root".into()), ..p(501, None, "f") };
        {
            let _lang = saysdf_i18n::testing::use_lang("en");
            assert_eq!(
                confirm_text(&foreign, Action::Suspend),
                format!("Suspend f (PID 501) owned by root? {}", crate::keys::CONFIRM_HINT)
            );
            assert_eq!(Action::Suspend.label(), "Pause");
        }
        let _lang = saysdf_i18n::testing::use_lang("ru");
        assert_eq!(
            confirm_text(&foreign, Action::Kill),
            format!("Завершить f (PID 501) пользователя root? {}", crate::keys::CONFIRM_HINT) // i18n-allow-cyrillic
        );
        assert_eq!(
            confirm_text(&foreign, Action::Suspend),
            format!("Приостановить f (PID 501) пользователя root? {}", crate::keys::CONFIRM_HINT) // i18n-allow-cyrillic
        );
        assert_eq!(Action::Suspend.label(), "Пауза"); // i18n-allow-cyrillic
    }
}
