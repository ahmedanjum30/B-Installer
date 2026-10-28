//! The installer script, `installer.toml`, as `bi` reads it and the setup gets it.
//!
//! `bi` fills in [`Manifest::packed`] and [`Manifest::license_text`], drops what only the build
//! needs, and stores the result as JSON in the setup's `MANIFEST` resource. The files themselves
//! follow in the `PAYLOAD` resource: one xz stream, in the order of `packed`.
//!
//! Paths the setup sees may use these constants, expanded at install time:
//! `{app}` the install folder, `{pf}` Program Files (or `%LOCALAPPDATA%\Programs` for a
//! per-user install), `{programs}` the Start menu's Programs folder, `{desktop}`,
//! `{localappdata}`, `{appdata}`, `{sys}` and `{tmp}`. Anything else in braces is left as is.

use serde::{Deserialize, Serialize};

/// Names of the setup's resources (type RT_RCDATA, language neutral).
pub const MANIFEST_RESOURCE: &str = "MANIFEST";
pub const PAYLOAD_RESOURCE: &str = "PAYLOAD";

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub app: App,
    /// Build time only: named lists for `each` in [`Registry`].
    #[serde(default, skip_serializing)]
    pub lists: std::collections::BTreeMap<String, Vec<String>>,
    /// Build time only: values for `${name}` in the script, overridden by `bi -D name=value`.
    #[serde(default, skip_serializing)]
    pub defines: std::collections::BTreeMap<String, String>,
    #[serde(default, rename = "task")]
    pub tasks: Vec<Task>,
    /// Build time only: what to pack. The setup gets [`Manifest::packed`] instead.
    #[serde(default, rename = "file", skip_serializing)]
    pub files: Vec<File>,
    #[serde(default, rename = "shortcut")]
    pub shortcuts: Vec<Shortcut>,
    #[serde(default)]
    pub registry: Vec<Registry>,
    /// Folders added to PATH (the user's, or everyone's for an install for all users).
    #[serde(default)]
    pub path: Vec<PathEntry>,
    /// Programs run at the end of the install.
    #[serde(default)]
    pub run: Vec<Run>,
    /// Programs run first thing when uninstalling.
    #[serde(default)]
    pub uninstall_run: Vec<Run>,
    #[serde(default)]
    pub packed: Vec<Packed>,
    #[serde(default)]
    pub license_text: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct App {
    /// Never change it: Windows finds the existing install by it. An Inno Setup AppId (without
    /// its doubled `{`) carries over: the setup uses the same uninstall key, `{id}_is1`.
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub support_url: String,
    /// The default install folder, such as `{pf}\B-Rar`.
    pub dir: String,
    #[serde(default)]
    pub scope: Scope,
    /// Files deleted before installing, `*` and `?` allowed in the last part.
    #[serde(default)]
    pub delete: Vec<String>,
    /// The icon Add or remove programs shows, such as `{app}\B-Rar.exe`.
    #[serde(default)]
    pub uninstall_icon: Option<String>,
    /// Build time only: the setup's own icon (.ico).
    #[serde(default, skip_serializing)]
    pub icon: Option<String>,
    /// Build time only: a text file the setup links to.
    #[serde(default, skip_serializing)]
    pub license: Option<String>,
    /// Build time only: where to write the setup.
    #[serde(default, skip_serializing)]
    pub output: String,
}

/// Who the install is for.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// The current user, no admin prompt.
    User,
    /// Everyone on the PC (asks for admin).
    Machine,
    /// The current user, with a checkbox to install for everyone instead.
    #[default]
    Ask,
}

/// A checkbox on the setup's page. Entries with `task = "<name>"` only happen when it's ticked.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub name: String,
    pub label: String,
    #[serde(default = "yes")]
    pub checked: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct File {
    /// A file or a folder (packed with everything in it), relative to the script.
    pub source: String,
    /// The folder it goes in, such as `{app}`.
    pub dest: String,
    /// A different name for a single file.
    #[serde(default)]
    pub rename: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
}

/// One file in the payload.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Packed {
    /// The full destination, such as `{app}\extension\manifest.json`.
    pub dest: String,
    pub size: u64,
    #[serde(default)]
    pub task: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Shortcut {
    /// Without `.lnk`, such as `{programs}\B-Rar`.
    pub path: String,
    pub target: String,
    #[serde(default)]
    pub args: String,
    #[serde(default)]
    pub task: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    /// `HKCU`, `HKLM`, or `HKA`: HKLM for an install for all users, else HKCU.
    pub root: String,
    pub key: String,
    /// The value's name; none for the key's default value.
    #[serde(default)]
    pub name: Option<String>,
    pub value: RegValue,
    /// `expand` for REG_EXPAND_SZ.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub uninstall: Uninstall,
    #[serde(default)]
    pub task: Option<String>,
    /// Build time only: the name of a list in `[lists]`. The entry is repeated for each item,
    /// with `{each}` in `key`, `name` and `value` replaced by it.
    #[serde(default, skip_serializing)]
    pub each: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum RegValue {
    Dword(u32),
    String(String),
}

/// What uninstalling does with a registry entry. Keys left empty are removed either way.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Uninstall {
    /// Deletes the value.
    #[default]
    Value,
    /// Deletes the whole key.
    Key,
    /// Leaves it.
    Keep,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct PathEntry {
    pub dir: String,
    #[serde(default)]
    pub task: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub file: String,
    #[serde(default)]
    pub args: String,
    /// No console window.
    #[serde(default)]
    pub hidden: bool,
    /// Waits for it to finish. Ignored for `launch`.
    #[serde(default = "yes")]
    pub wait: bool,
    /// A "Launch <app>" checkbox at the end instead; skipped by silent installs.
    #[serde(default)]
    pub launch: bool,
    /// Only when the setup was started with this switch, such as `UPDATE=1` for `/UPDATE=1`.
    #[serde(default)]
    pub if_param: Option<String>,
    /// Runs as the user who started the setup, not as the administrator an install for all
    /// users runs as.
    #[serde(default)]
    pub as_user: bool,
    #[serde(default)]
    pub task: Option<String>,
}

fn yes() -> bool {
    true
}
