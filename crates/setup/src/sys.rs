//! The bits of Windows the setup needs: resources, folders, elevation, the registry,
//! shortcuts and processes.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM},
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::{
            Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile},
            Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS},
            LibraryLoader::{BeginUpdateResourceW, EndUpdateResourceW, FindResourceW, LoadResource, LockResource, SizeofResource, UpdateResourceW},
            Registry::*,
            Threading::{
                GetCurrentProcess, GetCurrentProcessId, GetExitCodeProcess, INFINITE, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess,
                WaitForSingleObject,
            },
        },
        UI::{
            Shell::*,
            WindowsAndMessaging::{
                HWND_BROADCAST, MB_ICONERROR, MESSAGEBOX_RESULT, MESSAGEBOX_STYLE, MessageBoxW, SMTO_ABORTIFHUNG, SW_SHOWNORMAL, SendMessageTimeoutW,
                WM_SETTINGCHANGE,
            },
        },
    },
    core::{GUID, HSTRING, Interface, PCWSTR, PWSTR, w},
};

const RT_RCDATA: PCWSTR = PCWSTR(10 as _);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn resource(name: &str) -> Option<&'static [u8]> {
    unsafe {
        let res = FindResourceW(None, &HSTRING::from(name), RT_RCDATA);
        if res.is_invalid() {
            return None;
        }
        let data = LockResource(LoadResource(None, res).ok()?);
        Some(std::slice::from_raw_parts(data.cast(), SizeofResource(None, res) as usize))
    }
}

/// Removes the manifest and payload from a copy of the setup, which makes it the uninstaller.
pub fn strip_payload(exe: &Path) -> Result<(), String> {
    unsafe {
        let h = BeginUpdateResourceW(&HSTRING::from(exe), false).map_err(|e| format!("{}: {e}", exe.display()))?;
        for name in [format::MANIFEST_RESOURCE, format::PAYLOAD_RESOURCE] {
            let _ = UpdateResourceW(h, RT_RCDATA, &HSTRING::from(name), 0, None, 0);
        }
        EndUpdateResourceW(h, false).map_err(|e| format!("{}: {e}", exe.display()))
    }
}

pub fn known_folder(id: &GUID) -> PathBuf {
    unsafe {
        match SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) {
            Ok(path) => {
                let s = path.to_string().unwrap_or_default();
                CoTaskMemFree(Some(path.0 as _));
                PathBuf::from(s)
            }
            Err(_) => PathBuf::new(),
        }
    }
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as _),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// Starts `exe` as administrator (the UAC prompt), and with `wait` returns its exit code.
pub fn run_elevated(exe: &Path, args: &str, wait: bool) -> Result<u32, String> {
    let file = HSTRING::from(exe);
    let params = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info).map_err(|_| "Administrator rights weren't granted.".to_string())?;
        let mut code = 0;
        if wait {
            WaitForSingleObject(info.hProcess, INFINITE);
            let _ = GetExitCodeProcess(info.hProcess, &mut code);
        }
        let _ = CloseHandle(info.hProcess);
        Ok(code)
    }
}

/// Runs `file args` (args as they'd be typed), without a console window if `hidden`.
pub fn run(file: &Path, args: &str, hidden: bool, wait: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(file);
    cmd.raw_arg(args);
    if hidden {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    if let Some(dir) = file.parent() {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", file.display()))?;
    if wait {
        let _ = child.wait();
    }
    Ok(())
}

/// Quotes an argument the way programs split their command line.
pub fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    // Backslashes are literal unless they come before a quote, where they're doubled.
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        let n = if c == '"' { slashes * 2 + 1 } else { slashes };
        out.extend(std::iter::repeat_n('\\', n));
        out.push(c);
        slashes = 0;
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// Ends every program running from `dir` (other than this one), so its files can be replaced.
pub fn close_apps_in(dir: &Path) {
    let prefix = format!("{}\\", dir.display().to_string().trim_end_matches('\\').to_lowercase());
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            if entry.th32ProcessID != GetCurrentProcessId()
                && let Ok(process) = OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
                    false,
                    entry.th32ProcessID,
                )
            {
                let mut buf = [0u16; 1024];
                let mut len = buf.len() as u32;
                if QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok()
                    && String::from_utf16_lossy(&buf[..len as usize]).to_lowercase().starts_with(&prefix)
                {
                    let _ = TerminateProcess(process, 1);
                    WaitForSingleObject(process, 5000);
                }
                let _ = CloseHandle(process);
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
}

pub fn create_shortcut(lnk: &Path, target: &Path, args: &str) -> Result<(), String> {
    let err = |e: windows::core::Error| format!("{}: {e}", lnk.display());
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(err)?;
        link.SetPath(&HSTRING::from(target)).map_err(err)?;
        link.SetArguments(&HSTRING::from(args)).map_err(err)?;
        if let Some(dir) = target.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir)).map_err(err)?;
        }
        link.cast::<IPersistFile>().map_err(err)?.Save(&HSTRING::from(lnk), true).map_err(err)
    }
}

pub fn message(text: &str, caption: &str, style: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(caption), style) }
}

pub fn error(text: &str, caption: &str) {
    message(text, caption, MB_ICONERROR);
}

/// Tells Explorer that file types changed (the right-click menu, "Open with").
pub fn notify_associations() {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) }
}

/// Tells running programs that PATH changed, so new terminals see it.
pub fn notify_environment() {
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(w!("Environment").as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            5000,
            None,
        );
    }
}

pub fn open(path: &Path) {
    unsafe {
        ShellExecuteW(Some(HWND::default()), w!("open"), &HSTRING::from(path), None, None, SW_SHOWNORMAL);
    }
}

// The registry.

pub fn root(name: &str) -> HKEY {
    if name == "HKLM" { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER }
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn create_key(root: HKEY, key: &str) -> Result<Key, String> {
    let mut h = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            root,
            &HSTRING::from(key),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_ALL_ACCESS,
            None,
            &mut h,
            None,
        )
    };
    status.ok().map_err(|e| format!("{key}: {e}"))?;
    Ok(Key(h))
}

fn open_key(root: HKEY, key: &str, access: REG_SAM_FLAGS) -> Option<Key> {
    let mut h = HKEY::default();
    unsafe { RegOpenKeyExW(root, &HSTRING::from(key), None, access, &mut h) }.ok().ok()?;
    Some(Key(h))
}

fn name_arg(name: Option<&str>) -> HSTRING {
    HSTRING::from(name.unwrap_or(""))
}

pub fn set_string(root: HKEY, key: &str, name: Option<&str>, value: &str, expand: bool) -> Result<(), String> {
    let k = create_key(root, key)?;
    let data: Vec<u8> = value.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    let kind = if expand { REG_EXPAND_SZ } else { REG_SZ };
    unsafe { RegSetValueExW(k.0, &name_arg(name), None, kind, Some(&data)) }
        .ok()
        .map_err(|e| format!("{key}: {e}"))
}

pub fn set_dword(root: HKEY, key: &str, name: Option<&str>, value: u32) -> Result<(), String> {
    let k = create_key(root, key)?;
    unsafe { RegSetValueExW(k.0, &name_arg(name), None, REG_DWORD, Some(&value.to_le_bytes())) }
        .ok()
        .map_err(|e| format!("{key}: {e}"))
}

pub fn get_string(root: HKEY, key: &str, name: Option<&str>) -> Option<String> {
    let k = open_key(root, key, KEY_READ)?;
    let name = name_arg(name);
    let mut size = 0u32;
    unsafe { RegQueryValueExW(k.0, &name, None, None, None, Some(&mut size)) }.ok().ok()?;
    let mut buf = vec![0u16; size as usize / 2 + 1];
    unsafe { RegQueryValueExW(k.0, &name, None, None, Some(buf.as_mut_ptr().cast()), Some(&mut size)) }
        .ok()
        .ok()?;
    buf.truncate(size as usize / 2);
    while buf.last() == Some(&0) {
        buf.pop();
    }
    Some(String::from_utf16_lossy(&buf))
}

pub fn delete_value(root: HKEY, key: &str, name: Option<&str>) {
    if let Some(k) = open_key(root, key, KEY_SET_VALUE) {
        unsafe {
            let _ = RegDeleteValueW(k.0, &name_arg(name));
        }
    }
}

pub fn delete_tree(root: HKEY, key: &str) {
    unsafe {
        let _ = RegDeleteTreeW(root, &HSTRING::from(key));
    }
}

/// Deletes `key` and then its parents while they're empty, short of a top-level key such as
/// `Software`. Shared keys like `Software\Classes` are never empty.
pub fn prune(root: HKEY, key: &str) {
    let mut key = key.trim_end_matches('\\').to_string();
    while key.contains('\\') {
        let Some(k) = open_key(root, &key, KEY_READ) else { return };
        let (mut subkeys, mut values) = (0u32, 0u32);
        let ok = unsafe {
            RegQueryInfoKeyW(
                k.0,
                None,
                None,
                None,
                Some(&mut subkeys),
                None,
                None,
                Some(&mut values),
                None,
                None,
                None,
                None,
            )
        }
        .is_ok();
        drop(k);
        if !ok || subkeys > 0 || values > 0 {
            return;
        }
        unsafe {
            let _ = RegDeleteKeyW(root, &HSTRING::from(key.as_str()));
        }
        key.truncate(key.rfind('\\').unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::quote;

    #[test]
    fn quotes_like_the_command_line_splits() {
        assert_eq!(quote("plain"), "plain");
        assert_eq!(quote(""), r#""""#);
        assert_eq!(quote(r"/DIR=C:\Program Files\B-Rar"), r#""/DIR=C:\Program Files\B-Rar""#);
        // A trailing backslash would otherwise escape the closing quote.
        assert_eq!(quote(r"C:\a b\"), r#""C:\a b\\""#);
        assert_eq!(quote(r#"say "hi""#), r#""say \"hi\"""#);
    }
}
