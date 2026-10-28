# B-Installer

A minimal Windows installer builder, for apps that outgrew a hand-rolled Inno Setup script.
You write an `installer.toml`, run `bi`, and get one `Setup.exe` with one plain page: the
folder, a few checkboxes, and Install.

```
cargo build --release
target\release\bi.exe installer\b-rar.toml -D version=1.0.0 -D bindir=..\target\quick
```

`bi.exe` needs `bi-setup.exe` next to it (both come from `cargo build --release`), or use
`--stub <path>`.

## What it does

- Per-user installs by default (no admin prompt), with an optional "for everyone" checkbox
  (`scope = "ask"`, or `"user"` / `"machine"`).
- Files, folders (packed with everything in them), shortcuts, registry entries, PATH, programs to
  run after installing or before uninstalling.
- Closes the app's running programs (anything running from the install folder) before
  replacing them.
- An uninstaller and an entry in Settings > Apps, with everything it installed logged next to it.
- A version block, so Properties and the UAC prompt say "My App Setup", not the file name.
- LZMA2 compression, stored as PE resources, so the setup can still be signed afterwards.

## Drop-in for Inno Setup

Updating an app that Inno Setup installed works without changing the app:

- Same uninstall key (`{AppId}_is1`): the new setup finds the old install and replaces it.
- Same uninstaller name (`unins000.exe`), which apps often check for to tell an installed copy.
- Same switches: `/SILENT`, `/VERYSILENT`, `/SUPPRESSMSGBOXES`, `/NORESTART`, `/DIR=`,
  `/TASKS=`, `/ALLUSERS`, `/CURRENTUSER`. Any other `/NAME=value` can be checked with
  `if_param` (such as `/UPDATE=1`).

## The script

See [examples/b-rar.toml](examples/b-rar.toml) and [examples/bdownloader.toml](examples/bdownloader.toml)
(ports of real Inno scripts), and [crates/format/src/lib.rs](crates/format/src/lib.rs) for
every field.

```toml
[defines]                      # ${name}, overridden by -D name=value
version = "0.0.0"

[app]
id = "3B7F0E52-6C1D-4A8E-9F25-B0D4E7A61C38"   # never change it
name = "My App"
version = "${version}"
publisher = "Me"
dir = '{pf}\My App'
icon = 'app.ico'               # the setup's icon; B-Installer's own if left out
license = 'LICENSE'
output = 'dist\MyApp-Setup-${version}.exe'

[[task]]
name = "desktopicon"
label = "Create a desktop shortcut"
checked = false

[[file]]
source = 'target\release\myapp.exe'
dest = '{app}'

[[shortcut]]
path = '{programs}\My App'
target = '{app}\myapp.exe'

[[shortcut]]
path = '{desktop}\My App'
target = '{app}\myapp.exe'
task = "desktopicon"

[[run]]
file = '{app}\myapp.exe'
launch = true                  # "Launch My App" at the end
```

Paths are relative to the script, including ones passed with `-D`. At install time, paths may use `{app}`, `{pf}`,
`{programs}`, `{desktop}`, `{localappdata}`, `{appdata}`, `{sys}` and `{tmp}`. In the registry,
`HKA` is HKLM for an install for everyone, else HKCU. `each = "<list>"` repeats a registry
entry for every item of a `[lists]` entry.

## Not there (on purpose)

No wizard pages, no scripting language, no translations, no reboot handling. The logo comes
from [assets/make-icon.ps1](assets/make-icon.ps1).
