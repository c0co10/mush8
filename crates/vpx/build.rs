// Mush8 - build.rs
// Constantin Clerc (c22dev) - 0.1
use std::path::{Path, PathBuf};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "c") {
            out.push(p);
        }
    }
}

fn main() {
    let root = Path::new("libvpx");
    println!("cargo:rerun-if-changed=libvpx");
    let mut files = Vec::new();
    sources(root, &mut files);
    files.sort();
    cc::Build::new()
        .files(&files)
        .include(root)
        .include(root.join("config"))
        .opt_level(3)
        .warnings(false)
        .flag_if_supported("-Wno-everything")
        .compile("vpx");
}
