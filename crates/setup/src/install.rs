//! Installing: the files, shortcuts, registry, PATH, the uninstaller and its entry in
//! Settings > Apps, then the programs to run.

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    io::Read,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use format::{Manifest, RegValue, Run, Uninstall};
use lzma_rust2::XzReader;
use serde::{Deserialize, Serialize};
use windows::Win32::{
    System::{
        Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
        Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
    },
    UI::Shell::*,
};

use crate::sys;

/// Named as Inno Setup names them, so the apps' "is this an installed copy" checks and
/// updates over an Inno install keep working.
pub const UNINSTALLER: &str = "unins000.exe";
pub const LOG: &str = "unins000.dat";

pub struct Options {
    pub dir: PathBuf,
    /// For everyone on the PC (HKLM, Program Files) rather than the current user.
    pub machine: bool,
    pub tasks: HashSet<String>,
    /// `/NAME=value` switches, names in upper case.
    pub params: HashMap<String, String>,
    /// This is the administrator copy an install for everyone started: `as_user` programs are
    /// left to the copy that started it.
    pub elevated_child: bool,
}

impl Options {
    pub fn wants(&self, task: &Option<String>) -> bool {
        task.as_ref().is_none_or(|t| self.tasks.contains(t))
    }

    /// Whether a run's `if_param` (such as `UPDATE=1`) matches the command line.
    fn param_matches(&self, run: &Run) -> bool {
        let Some(cond) = &run.if_param else { return true };
        let (name, value) = cond.split_once('=').unwrap_or((cond, ""));
        self.params.get(&name.to_uppercase()).is_some_and(|v| v.eq_ignore_ascii_case(value))
    }
}

/// What the uninstaller removes, kept next to it. Updates add to it.
#[derive(Serialize, Deserialize, Default)]
pub struct Log {
    pub name: String,
    pub id: String,
    pub machine: bool,
    pub tasks: Vec<String>,
    pub files: Vec<PathBuf>,
    /// Folders the install created, removed if empty.
    pub dirs: Vec<PathBuf>,
    pub shortcuts: Vec<PathBuf>,
    pub registry: Vec<RegEntry>,
    pub paths: Vec<String>,
    pub uninstall_run: Vec<LoggedRun>,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
pub struct RegEntry {
    pub root: String,
    pub key: String,
    pub name: Option<String>,
    /// Delete the whole key, not only the value.
    pub tree: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct LoggedRun {
    pub file: PathBuf,
    pub args: String,
    pub hidden: bool,
    pub wait: bool,
}

impl Log {
    pub fn read(dir: &Path) -> Option<Log> {
        serde_json::from_slice(&fs::read(dir.join(LOG)).ok()?).ok()
    }
}

fn push<T: PartialEq>(list: &mut Vec<T>, item: T) {
    if !list.contains(&item) {
        list.push(item);
    }
}

/// Expands `{app}` and the other folder constants.
pub struct Folders {
    vars: Vec<(&'static str, String)>,
}

impl Folders {
    pub fn new(app: &Path, machine: bool) -> Folders {
        let k = |id| sys::known_folder(id).display().to_string();
        let (pf, programs, desktop) = if machine {
            (k(&FOLDERID_ProgramFiles), k(&FOLDERID_CommonPrograms), k(&FOLDERID_PublicDesktop))
        } else {
            (k(&FOLDERID_UserProgramFiles), k(&FOLDERID_Programs), k(&FOLDERID_Desktop))
        };
        let tmp = std::env::temp_dir().display().to_string();
        Folders {
            vars: vec![
                ("{app}", app.display().to_string()),
                ("{pf}", pf),
                ("{programs}", programs),
                ("{desktop}", desktop),
                ("{localappdata}", k(&FOLDERID_LocalAppData)),
                ("{appdata}", k(&FOLDERID_RoamingAppData)),
                ("{sys}", k(&FOLDERID_System)),
                ("{tmp}", tmp.trim_end_matches('\\').to_string()),
            ],
        }
    }

    pub fn expand(&self, s: &str) -> String {
        let mut s = s.to_string();
        for (name, value) in &self.vars {
            s = s.replace(name, value);
        }
        s
    }

    pub fn path(&self, s: &str) -> PathBuf {
        PathBuf::from(self.expand(s))
    }
}

pub fn uninstall_key(id: &str) -> String {
    let id = id.trim_matches(['{', '}']);
    format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{{{id}}}_is1")
}

fn uninstall_root(machine: bool) -> HKEY {
    if machine { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER }
}

/// An install of this app already on the PC (by this setup or by Inno Setup).
pub struct Existing {
    pub dir: PathBuf,
    pub machine: bool,
    /// The tasks it was installed with, if this setup installed it.
    pub tasks: Option<Vec<String>>,
}

pub fn find_existing(id: &str) -> Option<Existing> {
    let key = uninstall_key(id);
    [(HKEY_CURRENT_USER, false), (HKEY_LOCAL_MACHINE, true)]
        .into_iter()
        .find_map(|(root, machine)| {
            let dir = PathBuf::from(sys::get_string(root, &key, Some("InstallLocation"))?.trim_end_matches('\\'));
            let tasks = Log::read(&dir).map(|log| log.tasks);
            Some(Existing { dir, machine, tasks })
        })
}

pub fn default_dir(m: &Manifest, machine: bool, existing: Option<&Existing>) -> PathBuf {
    match existing {
        Some(e) if e.machine == machine => e.dir.clone(),
        _ => Folders::new(Path::new(""), machine).path(&m.app.dir),
    }
}

/// Installs, reporting progress from 0 to 1.
pub fn install(m: &Manifest, payload: &[u8], o: &Options, progress: &mut dyn FnMut(f64)) -> Result<(), String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let app = o.dir.as_path();
    let f = Folders::new(app, o.machine);
    let ha = if o.machine { "HKLM" } else { "HKCU" };

    let mut log = Log::read(app).filter(|log| log.id == m.app.id).unwrap_or_default();
    log.name = m.app.name.clone();
    log.id = m.app.id.clone();
    log.machine = o.machine;
    log.tasks = o.tasks.iter().cloned().collect();
    log.tasks.sort();

    create_dirs(app, &mut log)?;
    sys::close_apps_in(app);
    for pattern in &m.app.delete {
        delete_matching(&f.path(pattern));
    }

    // The files, in the order bi packed them.
    let total = m.packed.iter().map(|p| p.size).sum::<u64>().max(1);
    let mut done = 0;
    let mut xz = XzReader::new(payload, false);
    for packed in &m.packed {
        let mut data = (&mut xz).take(packed.size);
        if o.wants(&packed.task) {
            let dest = f.path(&packed.dest);
            if let Some(dir) = dest.parent() {
                create_dirs(dir, &mut log)?;
            }
            write_file(&dest, &mut data)?;
            push(&mut log.files, dest);
        } else {
            io::copy(&mut data, &mut io::sink()).map_err(|e| e.to_string())?;
        }
        done += packed.size;
        progress(done as f64 / total as f64 * 0.9);
    }

    for s in m.shortcuts.iter().filter(|s| o.wants(&s.task)) {
        let lnk = PathBuf::from(f.expand(&s.path) + ".lnk");
        if let Some(dir) = lnk.parent() {
            create_dirs(dir, &mut log)?;
        }
        sys::create_shortcut(&lnk, &f.path(&s.target), &f.expand(&s.args))?;
        push(&mut log.shortcuts, lnk);
    }

    for r in m.registry.iter().filter(|r| o.wants(&r.task)) {
        let root = if r.root == "HKA" { ha } else { r.root.as_str() };
        let name = r.name.as_deref();
        match &r.value {
            RegValue::String(v) => sys::set_string(sys::root(root), &r.key, name, &f.expand(v), r.kind.as_deref() == Some("expand"))?,
            RegValue::Dword(v) => sys::set_dword(sys::root(root), &r.key, name, *v)?,
        }
        if r.uninstall != Uninstall::Keep {
            push(
                &mut log.registry,
                RegEntry {
                    root: root.to_string(),
                    key: r.key.clone(),
                    name: r.name.clone(),
                    tree: r.uninstall == Uninstall::Key,
                },
            );
        }
    }

    let mut path_changed = false;
    for p in m.path.iter().filter(|p| o.wants(&p.task)) {
        let dir = f.expand(&p.dir);
        path_changed |= add_to_path(o.machine, &dir)?;
        push(&mut log.paths, dir);
    }
    progress(0.95);

    log.uninstall_run = m
        .uninstall_run
        .iter()
        .filter(|r| o.wants(&r.task))
        .map(|r| LoggedRun {
            file: f.path(&r.file),
            args: f.expand(&r.args),
            hidden: r.hidden,
            wait: r.wait,
        })
        .collect();
    write_uninstaller(m, app, o.machine, &f, &log, total)?;

    sys::notify_associations();
    if path_changed {
        sys::notify_environment();
    }
    run_programs(m, o, |r| !r.launch && !(o.elevated_child && r.as_user));
    progress(1.0);
    Ok(())
}

/// Installs for everyone from a setup running as the user: runs a copy of this setup as
/// administrator, silently, then the `as_user` programs itself.
pub fn install_elevated(m: &Manifest, o: &Options) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let errfile = std::env::temp_dir().join(format!("bi-setup-{}.err", std::process::id()));
    let mut args = vec![
        "/VERYSILENT".to_string(),
        "/SUPPRESSMSGBOXES".into(),
        "/ELEVATED".into(),
        "/ALLUSERS".into(),
        format!("/DIR={}", o.dir.display()),
        format!("/TASKS={}", o.tasks.iter().cloned().collect::<Vec<_>>().join(",")),
        format!("/ERRFILE={}", errfile.display()),
    ];
    for (name, value) in &o.params {
        if !matches!(name.as_str(), "DIR" | "TASKS" | "ERRFILE") {
            args.push(format!("/{name}={value}"));
        }
    }
    let args = args.iter().map(|a| sys::quote(a)).collect::<Vec<_>>().join(" ");
    let code = sys::run_elevated(&exe, &args, true)?;
    if code != 0 {
        let message = fs::read_to_string(&errfile).unwrap_or_else(|_| format!("Setup ended with code {code}."));
        let _ = fs::remove_file(&errfile);
        return Err(message);
    }
    run_programs(m, o, |r| !r.launch && r.as_user);
    Ok(())
}

/// Runs the `[[run]]` entries `which` picks, whose task and `if_param` match.
pub fn run_programs(m: &Manifest, o: &Options, which: impl Fn(&Run) -> bool) {
    let f = Folders::new(&o.dir, o.machine);
    for r in m.run.iter().filter(|r| which(r) && o.wants(&r.task) && o.param_matches(r)) {
        // A program that fails to start isn't worth failing the install for.
        let _ = sys::run(&f.path(&r.file), &f.expand(&r.args), r.hidden, r.wait && !r.launch);
    }
}

pub fn has_launch(m: &Manifest, o: &Options) -> bool {
    m.run.iter().any(|r| r.launch && o.wants(&r.task))
}

/// Creates `dir` and its missing parents, noting the ones it created.
fn create_dirs(dir: &Path, log: &mut Log) -> Result<(), String> {
    let missing: Vec<_> = dir.ancestors().take_while(|d| !d.exists()).map(Path::to_path_buf).collect();
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for d in missing {
        push(&mut log.dirs, d);
    }
    Ok(())
}

/// Writes a file, retrying for a few seconds while something still has it open.
fn write_file(dest: &Path, data: &mut impl Read) -> Result<(), String> {
    let mut tries = 0;
    let mut file = loop {
        match fs::File::create(dest) {
            Ok(file) => break file,
            Err(_) if tries < 20 => {
                tries += 1;
                thread::sleep(Duration::from_millis(250));
            }
            Err(e) => return Err(format!("{}: {e}", dest.display())),
        }
    };
    io::copy(data, &mut io::BufWriter::new(&mut file)).map_err(|e| format!("{}: {e}", dest.display()))?;
    Ok(())
}

/// Deletes the files matching `pattern`: `*` and `?` in its last part.
fn delete_matching(pattern: &Path) {
    let (Some(dir), Some(name)) = (pattern.parent(), pattern.file_name()) else {
        return;
    };
    let name = name.to_string_lossy().to_lowercase();
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if entry.path().is_file() && wildcard(&name, &entry.file_name().to_string_lossy().to_lowercase()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn wildcard(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    fn go(p: &[char], n: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|i| go(&p[1..], &n[i..])),
            Some('?') => !n.is_empty() && go(&p[1..], &n[1..]),
            Some(c) => n.first() == Some(c) && go(&p[1..], &n[1..]),
        }
    }
    go(&p, &n)
}

pub fn env_key(machine: bool) -> (HKEY, &'static str) {
    if machine {
        (HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
    } else {
        (HKEY_CURRENT_USER, "Environment")
    }
}

fn same_dir(a: &str, b: &str) -> bool {
    a.trim_end_matches('\\').eq_ignore_ascii_case(b.trim_end_matches('\\'))
}

/// Adds `dir` to PATH; false if it was already there.
fn add_to_path(machine: bool, dir: &str) -> Result<bool, String> {
    let (root, key) = env_key(machine);
    let path = sys::get_string(root, key, Some("Path")).unwrap_or_default();
    if path.split(';').any(|p| same_dir(p, dir)) {
        return Ok(false);
    }
    let path = if path.is_empty() || path.ends_with(';') {
        format!("{path}{dir}")
    } else {
        format!("{path};{dir}")
    };
    sys::set_string(root, key, Some("Path"), &path, true)?;
    Ok(true)
}

pub fn remove_from_path(machine: bool, dir: &str) {
    let (root, key) = env_key(machine);
    let Some(path) = sys::get_string(root, key, Some("Path")) else { return };
    let kept: Vec<_> = path.split(';').filter(|p| !same_dir(p, dir)).collect();
    if kept.len() != path.split(';').count() {
        let _ = sys::set_string(root, key, Some("Path"), &kept.join(";"), true);
    }
}

/// Writes unins000.exe (this setup without its payload), its log, and the app's entry in
/// Settings > Apps.
fn write_uninstaller(m: &Manifest, app: &Path, machine: bool, f: &Folders, log: &Log, size: u64) -> Result<(), String> {
    let exe = app.join(UNINSTALLER);
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    fs::copy(&me, &exe).map_err(|e| format!("{}: {e}", exe.display()))?;
    sys::strip_payload(&exe)?;
    // Inno Setup's message file, from an install this one replaces.
    let _ = fs::remove_file(app.join("unins000.msg"));
    let json = serde_json::to_vec_pretty(log).map_err(|e| e.to_string())?;
    fs::write(app.join(LOG), json).map_err(|e| format!("{LOG}: {e}"))?;

    let root = uninstall_root(machine);
    let key = uninstall_key(&m.app.id);
    // Starts clean, which also drops what Inno Setup kept there.
    sys::delete_tree(root, &key);
    let quoted = format!("\"{}\"", exe.display());
    let icon = m
        .app
        .uninstall_icon
        .as_deref()
        .map(|i| f.expand(i))
        .unwrap_or_else(|| exe.display().to_string());
    let strings = [
        ("DisplayName", m.app.name.clone()),
        ("DisplayVersion", m.app.version.clone()),
        ("Publisher", m.app.publisher.clone()),
        ("DisplayIcon", icon),
        ("InstallLocation", app.display().to_string()),
        ("UninstallString", quoted.clone()),
        ("QuietUninstallString", format!("{quoted} /VERYSILENT")),
        ("URLInfoAbout", m.app.url.clone()),
        ("HelpLink", m.app.support_url.clone()),
    ];
    for (name, value) in strings.iter().filter(|(_, v)| !v.is_empty()) {
        sys::set_string(root, &key, Some(name), value, false)?;
    }
    sys::set_dword(root, &key, Some("NoModify"), 1)?;
    sys::set_dword(root, &key, Some("NoRepair"), 1)?;
    sys::set_dword(root, &key, Some("EstimatedSize"), (size / 1024) as u32)?;
    Ok(())
}

/// Removes what the log lists. Run by the uninstaller.
pub fn uninstall(log: &Log, app: &Path) {
    sys::close_apps_in(app);
    for r in &log.uninstall_run {
        let _ = sys::run(&r.file, &r.args, r.hidden, r.wait);
    }
    for lnk in &log.shortcuts {
        let _ = fs::remove_file(lnk);
    }
    for r in log.registry.iter().rev() {
        let root = sys::root(&r.root);
        if r.tree {
            sys::delete_tree(root, &r.key);
        } else {
            sys::delete_value(root, &r.key, r.name.as_deref());
        }
        sys::prune(root, &r.key);
    }
    for dir in &log.paths {
        remove_from_path(log.machine, dir);
    }
    for file in &log.files {
        for _ in 0..10 {
            if fs::remove_file(file).is_ok() || !file.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
    }
    let _ = fs::remove_file(app.join(LOG));
    let _ = fs::remove_file(app.join(UNINSTALLER));
    let mut dirs = log.dirs.clone();
    push(&mut dirs, app.to_path_buf());
    // Deepest first, so parents are empty by the time they're tried.
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for dir in dirs {
        let _ = fs::remove_dir(dir);
    }
    sys::delete_tree(uninstall_root(log.machine), &uninstall_key(&log.id));
    sys::notify_associations();
    if !log.paths.is_empty() {
        sys::notify_environment();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard("*.old", "b-rar.exe.old"));
        assert!(wildcard("unins???.dat", "unins000.dat"));
        assert!(!wildcard("*.old", "b-rar.exe"));
        assert!(!wildcard("a?", "a"));
    }

    #[test]
    fn uninstall_key_takes_the_id_with_or_without_braces() {
        let key = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{AB-CD}_is1";
        assert_eq!(uninstall_key("AB-CD"), key);
        assert_eq!(uninstall_key("{AB-CD}"), key);
    }

    #[test]
    fn same_dir_ignores_case_and_trailing_backslashes() {
        assert!(same_dir(r"C:\Apps\B-Rar\", r"c:\apps\b-rar"));
        assert!(!same_dir(r"C:\Apps\B-Rar2", r"C:\Apps\B-Rar"));
    }
}
