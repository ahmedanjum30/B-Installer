//! The uninstaller, unins000.exe in the install folder. It can't delete itself while it runs,
//! so it asks first, then hands over to a copy of itself in %TEMP%, which removes the app and
//! then has cmd delete it once it has exited.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use windows::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONINFORMATION, MB_ICONQUESTION, MB_YESNO};

use crate::{
    Args,
    install::{self, Log},
    sys,
};

pub fn main(args: &Args) -> i32 {
    let Ok(me) = std::env::current_exe() else { return 1 };
    let quiet = args.silent() || args.flag("SUPPRESSMSGBOXES");

    // The copy in %TEMP%, already confirmed.
    if let Some(app) = args.params.get("APPDIR") {
        let app = PathBuf::from(app);
        let code = match Log::read(&app) {
            Some(log) => {
                install::uninstall(&log, &app);
                if !quiet {
                    sys::message(
                        &format!("{} was removed from this PC.", log.name),
                        &format!("Uninstall {}", log.name),
                        MB_ICONINFORMATION,
                    );
                }
                0
            }
            None => 1,
        };
        delete_later(&me);
        return code;
    }

    let Some(app) = me.parent().map(Path::to_path_buf) else { return 1 };
    let Some(log) = Log::read(&app) else {
        if !quiet {
            sys::error(
                &format!("{} is missing, so this can't be uninstalled.", app.join(install::LOG).display()),
                "Uninstall",
            );
        }
        return 1;
    };
    let caption = format!("Uninstall {}", log.name);
    if !args.silent() && sys::message(&format!("Remove {} from this PC?", log.name), &caption, MB_YESNO | MB_ICONQUESTION) != IDYES {
        return 0;
    }

    let copy = std::env::temp_dir().join(format!("bi-uninstall-{}.exe", std::process::id()));
    if let Err(e) = fs::copy(&me, &copy) {
        if !quiet {
            sys::error(&format!("Couldn't start the uninstaller: {e}"), &caption);
        }
        return 1;
    }
    let mut copy_args = vec![format!("/APPDIR={}", app.display())];
    copy_args.extend(args.flags.iter().map(|f| format!("/{f}")));
    let copy_args = copy_args.iter().map(|a| sys::quote(a)).collect::<Vec<_>>().join(" ");
    let started = if log.machine && !sys::is_elevated() {
        sys::run_elevated(&copy, &copy_args, false).map(|_| ())
    } else {
        sys::run(&copy, &copy_args, false, false)
    };
    if let Err(e) = started {
        let _ = fs::remove_file(&copy);
        if !quiet {
            sys::error(&e, &caption);
        }
        return 1;
    }
    0
}

/// Deletes this copy a moment after it exits.
fn delete_later(me: &Path) {
    use std::os::windows::process::CommandExt;
    let _ = Command::new("cmd.exe")
        .raw_arg(format!("/c ping 127.0.0.1 -n 3 >nul & del /f /q \"{}\"", me.display()))
        .creation_flags(0x0800_0000)
        .spawn();
}
