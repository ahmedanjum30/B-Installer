//! A setup bi made, or with the payload taken out, its uninstaller.
//!
//! Switches (Inno Setup's, so the apps' update code keeps working):
//!   /SILENT, /VERYSILENT  no window; tasks as last time, or their defaults
//!   /SUPPRESSMSGBOXES     no message boxes either
//!   /NORESTART            accepted; the setup never restarts Windows
//!   /DIR=<folder>         the install folder
//!   /TASKS=a,b            the ticked tasks
//!   /ALLUSERS, /CURRENTUSER
//!   /NAME=value           anything else, for `if_param` in [[run]]

#![windows_subsystem = "windows"]

mod install;
mod sys;
mod ui;
mod uninstall;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use format::{Manifest, Scope};

pub struct Args {
    /// `/NAME` switches, upper case.
    pub flags: HashSet<String>,
    /// `/NAME=value` switches, names in upper case.
    pub params: HashMap<String, String>,
}

impl Args {
    fn parse() -> Args {
        let mut args = Args {
            flags: HashSet::new(),
            params: HashMap::new(),
        };
        for arg in std::env::args().skip(1) {
            let Some(arg) = arg.strip_prefix('/').or_else(|| arg.strip_prefix('-')) else {
                continue;
            };
            if let Some((name, value)) = arg.split_once('=') {
                args.params.insert(name.to_uppercase(), value.to_string());
            } else {
                args.flags.insert(arg.to_uppercase());
            }
        }
        args
    }

    pub fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }

    pub fn silent(&self) -> bool {
        self.flag("SILENT") || self.flag("VERYSILENT") || self.flag("S")
    }
}

fn main() {
    let args = Args::parse();
    let code = match sys::resource(format::PAYLOAD_RESOURCE) {
        Some(payload) => setup(&args, payload),
        None => uninstall::main(&args),
    };
    std::process::exit(code);
}

fn setup(args: &Args, payload: &'static [u8]) -> i32 {
    let manifest = sys::resource(format::MANIFEST_RESOURCE).and_then(|m| serde_json::from_slice::<Manifest>(m).ok());
    let Some(m) = manifest else {
        sys::error("This setup is damaged. Download it again.", "Setup");
        return 1;
    };
    let existing = install::find_existing(&m.app.id);
    let machine = match m.app.scope {
        Scope::User => false,
        Scope::Machine => true,
        Scope::Ask if args.flag("ALLUSERS") => true,
        Scope::Ask if args.flag("CURRENTUSER") => false,
        Scope::Ask => existing.as_ref().is_some_and(|e| e.machine),
    };
    let dir = match args.params.get("DIR") {
        Some(dir) => PathBuf::from(dir.trim_end_matches('\\')),
        None => install::default_dir(&m, machine, existing.as_ref()),
    };
    let tasks = match (args.params.get("TASKS"), existing.as_ref().and_then(|e| e.tasks.as_ref())) {
        (Some(tasks), _) => tasks.split(',').filter(|t| !t.is_empty()).map(str::to_string).collect(),
        (None, Some(previous)) => previous.iter().cloned().collect(),
        (None, None) => m.tasks.iter().filter(|t| t.checked).map(|t| t.name.clone()).collect(),
    };
    let o = install::Options {
        dir,
        machine,
        tasks,
        params: args.params.clone(),
        elevated_child: args.flag("ELEVATED"),
    };
    if !args.silent() {
        return ui::run(m, payload, o, existing);
    }

    let result = if o.machine && !sys::is_elevated() {
        install::install_elevated(&m, &o)
    } else {
        install::install(&m, payload, &o, &mut |_| {})
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            if let Some(file) = args.params.get("ERRFILE") {
                let _ = std::fs::write(file, &e);
            } else if !args.flag("SUPPRESSMSGBOXES") {
                sys::error(&format!("Couldn't install {}.\n\n{e}", m.app.name), &format!("{} Setup", m.app.name));
            }
            1
        }
    }
}
