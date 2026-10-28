// Links setup.manifest: Common Controls v6 (the modern look, SysLink) and asInvoker, which also
// stops Windows from asking for admin just because the file name has "setup" in it. The icon is
// B-Installer's own, for setups whose script names none; bi swaps in the app's.
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=setup.manifest");
    println!("cargo:rerun-if-changed=../../assets/bi.ico");
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let path = |p: &str| dir.join(p).display().to_string().replace('\\', "/");
    // The icon is language neutral, as bi writes it, so the app's replaces it rather than sits
    // beside it. The manifest stays US English: neutral, ld can't merge it with std's default one.
    let rc = format!(
        "LANGUAGE 0, 0\n1 ICON \"{}\"\nLANGUAGE 9, 1\n1 24 \"{}\"\n",
        path("../../assets/bi.ico"),
        path("setup.manifest")
    );
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("setup.rc");
    fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE).manifest_required().unwrap();
}
