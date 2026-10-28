//! The setup's one window: the app's name, the folder, the tasks and Install. The same window
//! then shows progress and the result.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    thread,
};

use format::{Manifest, Scope};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{COLOR_GRAYTEXT, COLOR_WINDOW, CreateFontIndirectW, GetSysColor, GetSysColorBrush, HDC, HFONT, SetBkMode, SetTextColor, TRANSPARENT},
        System::{
            Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree},
            LibraryLoader::GetModuleHandleW,
        },
        UI::{
            Controls::{ICC_LINK_CLASS, ICC_PROGRESS_CLASS, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, NM_CLICK, NM_RETURN, NMHDR},
            HiDpi::GetDpiForSystem,
            Input::KeyboardAndMouse::{EnableWindow, SetFocus},
            Shell::{FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH},
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::{
    install::{self, Existing, Options},
    sys,
};

const ID_DIR: i32 = 10;
const ID_BROWSE: i32 = 11;
const ID_ALL_USERS: i32 = 12;
const ID_LAUNCH: i32 = 13;
const ID_LICENSE: i32 = 14;
const ID_TASK: i32 = 100;
const WM_PROGRESS: u32 = WM_APP + 1;
const WM_DONE: u32 = WM_APP + 2;
const DM_GETDEFID: u32 = WM_USER;
const BCM_SETSHIELD: u32 = 0x160C;
const PBM_SETRANGE32: u32 = WM_USER + 6;
const PBM_SETPOS: u32 = WM_USER + 2;
const PBM_SETMARQUEE: u32 = WM_USER + 10;
const PBS_MARQUEE: u32 = 0x08;
const BS_DEFPUSHBUTTON: u32 = 1;
const BS_AUTOCHECKBOX: u32 = 3;
const ES_AUTOHSCROLL: u32 = 0x80;
const SS_NOPREFIX: u32 = 0x80;
const BST_CHECKED: usize = 1;

/// Layout, in pixels at 100% scaling.
const WIDTH: i32 = 480;
const MARGIN: i32 = 24;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Ready,
    Busy,
    Done,
    Failed,
}

struct Ui {
    hwnd: HWND,
    m: Manifest,
    payload: &'static [u8],
    o: Options,
    existing: Option<Existing>,
    scale: f64,
    height: i32,
    state: State,
    title: HWND,
    subtitle: HWND,
    dir_label: HWND,
    dir: HWND,
    browse: HWND,
    checks: Vec<HWND>,
    all_users: Option<HWND>,
    status: HWND,
    progress: HWND,
    launch: HWND,
    ok: HWND,
    cancel: HWND,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    /// The subtitle, drawn in gray. Apart from UI so painting never needs to borrow it.
    static GRAY: Cell<isize> = const { Cell::new(0) };
    static EXIT_CODE: Cell<i32> = const { Cell::new(1) };
}

/// Runs `f` on the window's state, unless it's already in use further up the stack.
fn with<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|ui| ui.try_borrow_mut().ok()?.as_mut().map(f))
}

pub fn run(m: Manifest, payload: &'static [u8], o: Options, existing: Option<Existing>) -> i32 {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES | ICC_PROGRESS_CLASS | ICC_LINK_CLASS,
        });
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: w!("BInstallerSetup"),
            hbrBackground: GetSysColorBrush(COLOR_WINDOW),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            // The icon bi added, if any.
            hIcon: LoadIconW(Some(instance.into()), PCWSTR(1 as _)).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&class);
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let title = HSTRING::from(format!("{} Setup", m.app.name));
        let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), w!("BInstallerSetup"), &title, style, 0, 0, 0, 0, None, None, Some(instance.into()), None) else {
            return 1;
        };

        let mut ncm = NONCLIENTMETRICSW { cbSize: size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
        let _ = SystemParametersInfoW(SPI_GETNONCLIENTMETRICS, ncm.cbSize, Some(&mut ncm as *mut _ as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        let font = CreateFontIndirectW(&ncm.lfMessageFont);
        let mut big = ncm.lfMessageFont;
        big.lfHeight *= 2;
        big.lfWeight = 600;
        let big = CreateFontIndirectW(&big);

        let c = |class, text: &str, style: u32, id: i32| child(hwnd, class, text, style, id, font);
        let title = child(hwnd, w!("STATIC"), &m.app.name, SS_NOPREFIX, 0, big);
        let mut subtitle = format!("Version {}", m.app.version);
        if !m.app.publisher.is_empty() {
            subtitle += &format!("  ·  {}", m.app.publisher);
        }
        let subtitle = c(w!("STATIC"), &subtitle, SS_NOPREFIX, 0);
        GRAY.set(subtitle.0 as isize);
        let dir_label = c(w!("STATIC"), "Install to", SS_NOPREFIX, 0);
        let dir = child_ex(hwnd, WS_EX_CLIENTEDGE, w!("EDIT"), &o.dir.display().to_string(), ES_AUTOHSCROLL | WS_TABSTOP.0, ID_DIR, font);
        let browse = c(w!("BUTTON"), "Browse...", WS_TABSTOP.0, ID_BROWSE);
        let mut checks = Vec::new();
        for (i, task) in m.tasks.iter().enumerate() {
            let check = c(w!("BUTTON"), &task.label, BS_AUTOCHECKBOX | WS_TABSTOP.0, ID_TASK + i as i32);
            set_check(check, o.tasks.contains(&task.name));
            checks.push(check);
        }
        let all_users = (m.app.scope == Scope::Ask).then(|| {
            let check = c(w!("BUTTON"), "Install for everyone on this PC (asks for administrator rights)", BS_AUTOCHECKBOX | WS_TABSTOP.0, ID_ALL_USERS);
            set_check(check, o.machine);
            check
        });
        if m.license_text.is_some() {
            c(w!("SysLink"), "By installing, you agree to the <a>license</a>.", WS_TABSTOP.0, ID_LICENSE);
        }
        let status = c(w!("STATIC"), "", SS_NOPREFIX, 0);
        let progress = c(w!("msctls_progress32"), "", 0, 0);
        SendMessageW(progress, PBM_SETRANGE32, Some(WPARAM(0)), Some(LPARAM(1000)));
        let launch = c(w!("BUTTON"), &format!("Launch {}", m.app.name), BS_AUTOCHECKBOX | WS_TABSTOP.0, ID_LAUNCH);
        let ok = c(w!("BUTTON"), "Install", BS_DEFPUSHBUTTON | WS_TABSTOP.0, IDOK.0);
        let cancel = c(w!("BUTTON"), "Cancel", WS_TABSTOP.0, IDCANCEL.0);

        let scale = GetDpiForSystem() as f64 / 96.0;
        let rows = checks.len() as i32 + all_users.is_some() as i32;
        let height = (92 + 22 + 40 + 26 * rows).max(170) + 16 + 30 + 20;
        let shield = o.machine && !sys::is_elevated();
        let ui = Ui {
            hwnd, m, payload, o, existing, scale, height, state: State::Ready, title, subtitle,
            dir_label, dir, browse, checks, all_users, status, progress, launch, ok, cancel,
        };
        set_shield(ok, shield);
        layout(&ui);
        UI.with(|cell| *cell.borrow_mut() = Some(ui));

        // The window's size for the client area wanted, centred on the screen.
        let s = |v: i32| (v as f64 * scale).round() as i32;
        let mut rect = RECT { left: 0, top: 0, right: s(WIDTH), bottom: s(height) };
        let _ = AdjustWindowRectEx(&mut rect, style, false, WINDOW_EX_STYLE(0));
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let _ = SetWindowPos(hwnd, None, (sw - w) / 2, (sh - h) / 2, w, h, SWP_NOZORDER);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetFocus(Some(ok));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    EXIT_CODE.get()
}

fn child(parent: HWND, class: PCWSTR, text: &str, style: u32, id: i32, font: HFONT) -> HWND {
    child_ex(parent, WINDOW_EX_STYLE(0), class, text, style, id, font)
}

fn child_ex(parent: HWND, ex: WINDOW_EX_STYLE, class: PCWSTR, text: &str, style: u32, id: i32, font: HFONT) -> HWND {
    unsafe {
        let style = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(style);
        let menu = HMENU(id as isize as _);
        let h = CreateWindowExW(ex, class, &HSTRING::from(text), style, 0, 0, 0, 0, Some(parent), Some(menu), None, None).unwrap_or_default();
        SendMessageW(h, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        h
    }
}

fn layout(ui: &Ui) {
    let s = |v: i32| (v as f64 * ui.scale).round() as i32;
    let place = |h: HWND, x: i32, y: i32, w: i32, height: i32| unsafe {
        let _ = SetWindowPos(h, None, s(x), s(y), s(w), s(height), SWP_NOZORDER | SWP_NOACTIVATE);
    };
    let show = |h: HWND, visible: bool| unsafe {
        let _ = ShowWindow(h, if visible { SW_SHOW } else { SW_HIDE });
    };
    let inner = WIDTH - 2 * MARGIN;
    let hwnd = ui.hwnd;
    let item = |id: i32| unsafe { GetDlgItem(Some(hwnd), id).unwrap_or_default() };

    place(ui.title, MARGIN, 18, inner, 40);
    place(ui.subtitle, MARGIN, 58, inner, 20);

    let ready = ui.state == State::Ready;
    for h in [ui.dir_label, ui.dir, ui.browse].into_iter().chain(ui.checks.iter().copied()).chain(ui.all_users) {
        show(h, ready);
    }
    show(ui.status, !ready);
    show(ui.progress, ui.state == State::Busy);
    show(ui.launch, ui.state == State::Done && install::has_launch(&ui.m, &ui.o));

    let mut y = 96;
    if ready {
        place(ui.dir_label, MARGIN, y, inner, 20);
        y += 22;
        place(ui.dir, MARGIN, y, inner - 92, 26);
        place(ui.browse, WIDTH - MARGIN - 84, y - 1, 84, 28);
        y += 42;
        for &h in ui.checks.iter().chain(&ui.all_users) {
            place(h, MARGIN, y, inner, 24);
            y += 26;
        }
    } else {
        let lines = if ui.state == State::Failed { 80 } else { 20 };
        place(ui.status, MARGIN, y, inner, lines);
        y += 30;
        place(ui.progress, MARGIN, y, inner, 12);
        place(ui.launch, MARGIN, y, inner, 24);
    }

    let footer = ui.height - 20 - 30;
    let license = item(ID_LICENSE);
    show(license, ready);
    place(license, MARGIN, footer + 6, 230, 20);
    let cancel = ui.state == State::Ready || ui.state == State::Busy;
    show(ui.cancel, cancel);
    place(ui.cancel, WIDTH - MARGIN - 96, footer, 96, 30);
    let ok_x = if cancel { WIDTH - MARGIN - 96 * 2 - 8 } else { WIDTH - MARGIN - 96 };
    place(ui.ok, ok_x, footer, 96, 30);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_CTLCOLORSTATIC => {
                let hdc = HDC(wparam.0 as _);
                SetBkMode(hdc, TRANSPARENT);
                if lparam.0 == GRAY.get() {
                    SetTextColor(hdc, windows::Win32::Foundation::COLORREF(GetSysColor(COLOR_GRAYTEXT)));
                }
                LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize)
            }
            WM_COMMAND => {
                command(hwnd, (wparam.0 & 0xFFFF) as i32);
                LRESULT(0)
            }
            WM_NOTIFY => {
                let hdr = &*(lparam.0 as *const NMHDR);
                if hdr.idFrom == ID_LICENSE as usize && (hdr.code == NM_CLICK || hdr.code == NM_RETURN) {
                    show_license();
                }
                LRESULT(0)
            }
            // For IsDialogMessage: Enter presses the default button.
            DM_GETDEFID => LRESULT((0x534B << 16) | IDOK.0 as isize),
            WM_PROGRESS => {
                with(|ui| SendMessageW(ui.progress, PBM_SETPOS, Some(WPARAM(wparam.0)), None));
                LRESULT(0)
            }
            WM_DONE => {
                let result = *Box::from_raw(lparam.0 as *mut Result<(), String>);
                done(result);
                LRESULT(0)
            }
            WM_CLOSE => {
                if with(|ui| ui.state) != Some(State::Busy) {
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn command(hwnd: HWND, id: i32) {
    let state = with(|ui| ui.state);
    match id {
        _ if id == IDOK.0 => match state {
            Some(State::Ready) => start(),
            Some(State::Done) => {
                with(|ui| {
                    if ui.launch_checked() {
                        install::run_programs(&ui.m, &ui.o, |r| r.launch);
                    }
                });
                EXIT_CODE.set(0);
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            Some(State::Failed) => unsafe {
                let _ = DestroyWindow(hwnd);
            },
            _ => {}
        },
        _ if id == IDCANCEL.0 => {
            if state != Some(State::Busy) {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
        }
        ID_BROWSE => browse(),
        ID_ALL_USERS => {
            with(|ui| {
                let machine = ui.all_users.is_some_and(checked);
                let old = install::default_dir(&ui.m, !machine, ui.existing.as_ref());
                if std::path::Path::new(&text(ui.dir)) == old {
                    let new = install::default_dir(&ui.m, machine, ui.existing.as_ref());
                    set_text(ui.dir, &new.display().to_string());
                }
                set_shield(ui.ok, machine && !sys::is_elevated());
            });
        }
        _ => {}
    }
}

impl Ui {
    fn launch_checked(&self) -> bool {
        install::has_launch(&self.m, &self.o) && checked(self.launch)
    }
}

fn start() {
    let job = with(|ui| {
        let dir = PathBuf::from(text(ui.dir).trim().trim_end_matches('\\'));
        if !dir.is_absolute() {
            sys::error("Choose the folder to install to.", &format!("{} Setup", ui.m.app.name));
            return None;
        }
        ui.o.dir = dir;
        ui.o.tasks = ui.m.tasks.iter().zip(&ui.checks).filter(|(_, c)| checked(**c)).map(|(t, _)| t.name.clone()).collect();
        ui.o.machine = match ui.all_users {
            Some(c) => checked(c),
            None => ui.m.app.scope == Scope::Machine,
        };
        ui.state = State::Busy;
        set_text(ui.status, &format!("Installing {}...", ui.m.app.name));
        let elevate = ui.o.machine && !sys::is_elevated();
        if elevate {
            // No progress comes back from the administrator copy.
            unsafe {
                let style = GetWindowLongPtrW(ui.progress, GWL_STYLE);
                SetWindowLongPtrW(ui.progress, GWL_STYLE, style | PBS_MARQUEE as isize);
                SendMessageW(ui.progress, PBM_SETMARQUEE, Some(WPARAM(1)), Some(LPARAM(30)));
            }
        }
        unsafe {
            let _ = EnableWindow(ui.ok, false);
            let _ = EnableWindow(ui.cancel, false);
        }
        layout(ui);
        let o = Options {
            dir: ui.o.dir.clone(),
            machine: ui.o.machine,
            tasks: ui.o.tasks.clone(),
            params: ui.o.params.clone(),
            elevated_child: ui.o.elevated_child,
        };
        Some((ui.hwnd.0 as isize, ui.m.clone(), ui.payload, o, elevate))
    });
    let Some(Some((hwnd, m, payload, o, elevate))) = job else { return };
    thread::spawn(move || {
        let post = |msg, w: usize, l: isize| unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as _)), msg, WPARAM(w), LPARAM(l));
        };
        let result = if elevate {
            install::install_elevated(&m, &o)
        } else {
            install::install(&m, payload, &o, &mut |p| post(WM_PROGRESS, (p * 1000.0) as usize, 0))
        };
        post(WM_DONE, 0, Box::into_raw(Box::new(result)) as isize);
    });
}

fn done(result: Result<(), String>) {
    with(|ui| {
        let name = &ui.m.app.name;
        match result {
            Ok(()) => {
                ui.state = State::Done;
                set_text(ui.status, &format!("{name} is installed."));
                set_text(ui.ok, "Finish");
                set_check(ui.launch, true);
            }
            Err(e) => {
                ui.state = State::Failed;
                set_text(ui.status, &format!("Couldn't install {name}.\n\n{e}"));
                set_text(ui.ok, "Close");
            }
        }
        set_shield(ui.ok, false);
        unsafe {
            let _ = EnableWindow(ui.ok, true);
            let _ = SetFocus(Some(ui.ok));
        }
        layout(ui);
    });
}

fn browse() {
    // Read what's needed, then let go of the state: the dialog runs its own message loop.
    let Some((hwnd, dir, current, leaf)) = with(|ui| {
        let leaf = install::default_dir(&ui.m, ui.o.machine, None).file_name().map(|n| n.to_os_string());
        (ui.hwnd, ui.dir, PathBuf::from(text(ui.dir)), leaf)
    }) else {
        return;
    };
    let Some(mut picked) = pick_folder(hwnd, &current) else { return };
    // As Inno Setup does: a folder picked for the app gets the app's own folder inside it.
    if let Some(leaf) = leaf
        && picked.file_name() != Some(leaf.as_os_str())
    {
        picked.push(leaf);
    }
    set_text(dir, &picked.display().to_string());
}

fn pick_folder(owner: HWND, current: &std::path::Path) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dialog.SetOptions(dialog.GetOptions().ok()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        if let Some(start) = current.ancestors().find(|p| p.is_dir())
            && let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(start), None)
        {
            let _ = dialog.SetFolder(&item);
        }
        dialog.Show(Some(owner)).ok()?;
        let name = dialog.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as _));
        path.map(PathBuf::from)
    }
}

fn show_license() {
    with(|ui| {
        let Some(text) = &ui.m.license_text else { return };
        let path = std::env::temp_dir().join(format!("{} license.txt", ui.m.app.name));
        if std::fs::write(&path, text.replace("\r\n", "\n").replace('\n', "\r\n")).is_ok() {
            sys::open(&path);
        }
    });
}

fn text(h: HWND) -> String {
    let mut buf = [0u16; 1024];
    let len = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

fn set_text(h: HWND, s: &str) {
    unsafe {
        let _ = SetWindowTextW(h, &HSTRING::from(s));
    }
}

fn checked(h: HWND) -> bool {
    unsafe { SendMessageW(h, BM_GETCHECK, None, None).0 as usize == BST_CHECKED }
}

fn set_check(h: HWND, on: bool) {
    unsafe {
        SendMessageW(h, BM_SETCHECK, Some(WPARAM(on as usize)), None);
    }
}

fn set_shield(button: HWND, on: bool) {
    unsafe {
        SendMessageW(button, BCM_SETSHIELD, Some(WPARAM(0)), Some(LPARAM(on as isize)));
    }
}
