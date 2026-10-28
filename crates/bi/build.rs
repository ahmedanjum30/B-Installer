// B-Installer's icon on bi.exe.
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=../../assets/bi.ico");
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = dir.join("../../assets/bi.ico").display().to_string().replace('\\', "/");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bi.rc");
    fs::write(&out, format!("1 ICON \"{icon}\"\n")).unwrap();
    embed_resource::compile(&out, embed_resource::NONE).manifest_optional().unwrap();
}
