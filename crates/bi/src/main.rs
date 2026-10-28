//! Builds a setup from an installer script:
//!
//!   bi installer.toml [-D name=value]... [--stub bi-setup.exe]
//!
//! The setup is a copy of bi-setup.exe (next to bi.exe unless --stub says otherwise) with the
//! script, the files and the icon added as resources. See crates\format for the script.

mod resources;

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
};

use format::{Manifest, Packed};
use lzma_rust2::{FilterType, XzOptions, XzWriter};
use serde::Deserialize;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bi: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut script = None;
    let mut stub = None;
    let mut defines = BTreeMap::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-D" => {
                let define = args.next().ok_or("-D needs name=value")?;
                let (name, value) = define.split_once('=').ok_or("-D needs name=value")?;
                defines.insert(name.to_string(), value.to_string());
            }
            "--stub" => stub = Some(PathBuf::from(args.next().ok_or("--stub needs a path")?)),
            "-h" | "--help" => {
                println!("usage: bi installer.toml [-D name=value]... [--stub bi-setup.exe]");
                return Ok(());
            }
            _ if script.is_none() => script = Some(PathBuf::from(arg)),
            _ => return Err(format!("unexpected argument {arg}")),
        }
    }
    let script = script.ok_or("usage: bi installer.toml [-D name=value]... [--stub bi-setup.exe]")?;
    let stub = match stub {
        Some(stub) => stub,
        None => std::env::current_exe().map_err(|e| e.to_string())?.with_file_name("bi-setup.exe"),
    };
    let base = script.parent().unwrap_or(Path::new(".")).to_path_buf();

    let mut m = load(&script, defines)?;
    expand_each(&mut m)?;
    check_tasks(&m)?;
    if let Some(license) = &m.app.license {
        let path = base.join(license);
        m.license_text = Some(fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?);
    }
    let files = collect(&m, &base)?;
    m.packed = files.iter().map(|(_, packed)| packed.clone()).collect();

    let total: u64 = m.packed.iter().map(|p| p.size).sum();
    println!("Packing {} files ({:.1} MB)...", files.len(), total as f64 / 1e6);
    let payload = compress(&files)?;
    let manifest = serde_json::to_vec(&m).map_err(|e| e.to_string())?;

    if m.app.output.is_empty() {
        return Err("app.output is missing".into());
    }
    let out = base.join(&m.app.output);
    if let Some(dir) = out.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::copy(&stub, &out).map_err(|e| format!("{}: {e} (build bi-setup first, or pass --stub)", stub.display()))?;
    let icon = match &m.app.icon {
        Some(icon) => {
            let path = base.join(icon);
            Some(fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)
        }
        None => None,
    };
    resources::write(&out, &m.app, icon.as_deref(), &manifest, &payload).inspect_err(|_| {
        let _ = fs::remove_file(&out);
    })?;

    let size = fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    println!("Built {} ({:.2} MB)", out.display(), size as f64 / 1e6);
    Ok(())
}

/// Reads the script, with `${name}` in its strings replaced from `[defines]` and `-D`.
fn load(script: &Path, cli: BTreeMap<String, String>) -> Result<Manifest, String> {
    let text = fs::read_to_string(script).map_err(|e| format!("{}: {e}", script.display()))?;
    let mut value: toml::Value = toml::from_str(&text).map_err(|e| format!("{}: {e}", script.display()))?;
    let mut defines = BTreeMap::new();
    if let Some(table) = value.get("defines").and_then(|d| d.as_table()) {
        for (name, v) in table {
            let v = v.as_str().ok_or_else(|| format!("defines.{name} must be a string"))?;
            defines.insert(name.clone(), v.to_string());
        }
    }
    defines.extend(cli);
    substitute(&mut value, &defines)?;
    Manifest::deserialize(value).map_err(|e| format!("{}: {e}", script.display()))
}

fn substitute(value: &mut toml::Value, defines: &BTreeMap<String, String>) -> Result<(), String> {
    match value {
        toml::Value::String(s) => {
            let mut out = String::new();
            let mut rest = s.as_str();
            while let Some(start) = rest.find("${") {
                out.push_str(&rest[..start]);
                let end = rest[start..].find('}').ok_or_else(|| format!("unclosed ${{ in \"{s}\""))?;
                let name = &rest[start + 2..start + end];
                let v = defines
                    .get(name)
                    .ok_or_else(|| format!("${{{name}}} isn't defined (add it to [defines] or pass -D {name}=...)"))?;
                out.push_str(v);
                rest = &rest[start + end + 1..];
            }
            out.push_str(rest);
            *s = out;
        }
        toml::Value::Array(items) => items.iter_mut().try_for_each(|v| substitute(v, defines))?,
        toml::Value::Table(table) => table.iter_mut().try_for_each(|(_, v)| substitute(v, defines))?,
        _ => {}
    }
    Ok(())
}

/// Repeats each registry entry with `each` for every item of its list.
fn expand_each(m: &mut Manifest) -> Result<(), String> {
    let mut registry = Vec::new();
    for entry in m.registry.drain(..) {
        let Some(list) = &entry.each else {
            registry.push(entry);
            continue;
        };
        let items = m.lists.get(list).ok_or_else(|| format!("no list \"{list}\" in [lists]"))?;
        for item in items {
            let mut e = entry.clone();
            e.each = None;
            e.key = e.key.replace("{each}", item);
            e.name = e.name.map(|n| n.replace("{each}", item));
            if let format::RegValue::String(v) = &mut e.value {
                *v = v.replace("{each}", item);
            }
            registry.push(e);
        }
    }
    m.registry = registry;
    Ok(())
}

fn check_tasks(m: &Manifest) -> Result<(), String> {
    let used = m
        .files
        .iter()
        .map(|f| &f.task)
        .chain(m.shortcuts.iter().map(|s| &s.task))
        .chain(m.registry.iter().map(|r| &r.task))
        .chain(m.path.iter().map(|p| &p.task))
        .chain(m.run.iter().map(|r| &r.task));
    for task in used.flatten() {
        if !m.tasks.iter().any(|t| &t.name == task) {
            return Err(format!("task \"{task}\" isn't declared with [[task]]"));
        }
    }
    Ok(())
}

/// Every file to pack: its source and where it goes.
fn collect(m: &Manifest, base: &Path) -> Result<Vec<(PathBuf, Packed)>, String> {
    let mut out = Vec::new();
    for f in &m.files {
        let source = base.join(&f.source);
        let meta = fs::metadata(&source).map_err(|e| format!("{}: {e}", source.display()))?;
        if meta.is_dir() {
            if f.rename.is_some() {
                return Err(format!("{}: rename only works for a file", f.source));
            }
            let mut found = Vec::new();
            walk(&source, &mut found)?;
            for path in found {
                let rel = path.strip_prefix(&source).unwrap().to_string_lossy().replace('/', "\\");
                let size = fs::metadata(&path).map_err(|e| e.to_string())?.len();
                out.push((
                    path,
                    Packed {
                        dest: format!("{}\\{rel}", f.dest),
                        size,
                        task: f.task.clone(),
                    },
                ));
            }
        } else {
            let name = match &f.rename {
                Some(name) => name.clone(),
                None => source.file_name().unwrap().to_string_lossy().into_owned(),
            };
            out.push((
                source,
                Packed {
                    dest: format!("{}\\{name}", f.dest),
                    size: meta.len(),
                    task: f.task.clone(),
                },
            ));
        }
    }
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() { walk(&path, out)? } else { out.push(path) }
    }
    Ok(())
}

/// All the files as one xz stream (LZMA2, with the x86 filter for the executables).
fn compress(files: &[(PathBuf, Packed)]) -> Result<Vec<u8>, String> {
    let mut options = XzOptions::with_preset(9);
    options.prepend_pre_filter(FilterType::BcjX86, 0);
    let mut xz = XzWriter::new(Vec::new(), options).map_err(|e| e.to_string())?;
    for (path, _) in files {
        let data = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        xz.write_all(&data).map_err(|e| e.to_string())?;
    }
    xz.finish().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_defines_in_every_string() {
        let mut value: toml::Value = toml::from_str("a = 'x${v}y'\n[t]\nb = ['${v}', '{app}']").unwrap();
        substitute(&mut value, &BTreeMap::from([("v".to_string(), "1.2".to_string())])).unwrap();
        assert_eq!(value["a"].as_str(), Some("x1.2y"));
        assert_eq!(value["t"]["b"][0].as_str(), Some("1.2"));
        // Install-time constants stay.
        assert_eq!(value["t"]["b"][1].as_str(), Some("{app}"));
    }

    #[test]
    fn undefined_names_are_errors() {
        let mut value: toml::Value = toml::from_str("a = '${nope}'").unwrap();
        assert!(substitute(&mut value, &BTreeMap::new()).is_err());
    }
}
