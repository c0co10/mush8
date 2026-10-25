use anyhow::{Context, Result, bail};
use nxfs::storage::{DynStorage, HostFile, Mem};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const ARCHIVES: &[&str] = &["sarc", "szs", "bntx", "msbt"];

enum Node {
    File(PathBuf),
    Archive(BTreeMap<String, Node>),
}

struct Change<'a> {
    by: &'a str,
    node: Node,
}

fn child(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.is_dir() && p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

fn is_archive(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| ARCHIVES.iter().any(|a| a.eq_ignore_ascii_case(ext)))
}

fn entries(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .with_context(|| dir.display().to_string())?
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .filter(|(n, _)| !n.starts_with('.'))
        .collect();
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, Node)>) -> Result<()> {
    for (name, path) in entries(dir)? {
        let key = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        if path.is_dir() && is_archive(&name) {
            out.push((key, Node::Archive(archive(&path)?)));
        } else if path.is_dir() {
            walk(&path, &key, out)?;
        } else {
            out.push((key, Node::File(path)));
        }
    }
    Ok(())
}

fn archive(dir: &Path) -> Result<BTreeMap<String, Node>> {
    let mut out = Vec::new();
    walk(dir, "", &mut out)?;
    Ok(out.into_iter().collect())
}

fn png(path: &Path) -> Result<(Vec<u8>, u32, u32)> {
    let mut d = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path)?));
    d.set_transformations(png::Transformations::normalize_to_color8());
    let mut r = d.read_info()?;
    let mut buf = vec![0; r.output_buffer_size().context("PNG too large")?];
    let info = r.next_frame(&mut buf)?;
    let px = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        c => bail!("unsupported PNG color type {c:?}"),
    };
    Ok((rgba, info.width, info.height))
}

fn edit(data: &[u8], edits: &BTreeMap<String, Node>) -> Result<Vec<u8>> {
    if data.starts_with(b"Yaz0") {
        let inner = edit(&nxfs::sarc::yaz0(data)?, edits)?;
        return Ok(nxfs::sarc::yaz0_compress(&inner, nxfs::sarc::yaz0_tail(data)));
    }
    if data.starts_with(b"SARC") {
        let entries = nxfs::sarc::sarc(data)?;
        let mut contents: Vec<Vec<u8>> = entries.iter().map(|e| e.data.to_vec()).collect();
        for (key, node) in edits {
            let i = entries.iter().position(|e| e.is(key)).with_context(|| format!("the archive has no {key}"))?;
            contents[i] = match node {
                Node::File(p) => std::fs::read(p)?,
                Node::Archive(inner) => edit(&contents[i], inner).with_context(|| key.clone())?,
            };
        }
        return nxfs::sarc::rebuild(data, &contents.iter().map(Vec::as_slice).collect::<Vec<_>>());
    }
    if data.starts_with(b"BNTX") {
        let textures = nxfs::bntx::textures(data)?;
        let mut out = data.to_vec();
        for (key, node) in edits {
            let Node::File(p) = node else { bail!("{key}: a texture must be a .png file") };
            let stem = key.strip_suffix(".png").or_else(|| key.strip_suffix(".PNG")).with_context(|| format!("{key}: a texture must be a .png file"))?;
            let t = textures.iter().find(|t| t.name == stem || t.name.split('^').next() == Some(stem)).with_context(|| format!("the texture pack has no {stem}"))?;
            let (rgba, w, h) = png(p).with_context(|| key.clone())?;
            t.write(&mut out, &rgba, w, h)?;
        }
        return Ok(out);
    }
    if data.starts_with(b"MsgStdBn") {
        let mut m = nxfs::msbt::Msbt::parse(data)?;
        for (key, node) in edits {
            let Node::File(p) = node else { bail!("{key}: a text must be a .txt file") };
            let label = key.strip_suffix(".txt").with_context(|| format!("{key}: a text must be a .txt file"))?;
            let text = std::fs::read_to_string(p)?;
            m.set_text(label, text.trim_end_matches(['\r', '\n']))?;
        }
        return Ok(m.build());
    }
    bail!("Mush8 cannot edit inside this kind of file")
}

pub fn romfs(romfs: DynStorage, mods: &[PathBuf]) -> Result<DynStorage> {
    let mut changes: HashMap<String, (String, Vec<Change>)> = HashMap::new();
    let names: Vec<String> = mods.iter().map(|m| m.file_name().map_or_else(|| m.display().to_string(), |n| n.to_string_lossy().into_owned())).collect();
    for (m, name) in mods.iter().zip(&names) {
        let Some(dir) = child(m, "romfs") else { continue };
        let mut found = Vec::new();
        walk(&dir, "", &mut found)?;
        eprintln!("[mods] {name}: {} RomFS changes", found.len());
        for (path, node) in found {
            let path = format!("/{path}");
            changes.entry(path.to_ascii_lowercase()).or_insert_with(|| (path, Vec::new())).1.push(Change { by: name, node });
        }
    }
    if changes.is_empty() {
        return Ok(romfs);
    }
    let fs = nxfs::romfs::RomFs::open(romfs)?;
    let originals: HashMap<String, nxfs::romfs::File> = fs.files().into_iter().map(|f| (f.path.to_ascii_lowercase(), f)).collect();
    let mut overlay: Vec<(String, DynStorage)> = Vec::new();
    for (lower, (path, list)) in changes {
        let path = originals.get(&lower).map_or(path, |f| f.path.clone());
        let last_file = list.iter().rposition(|c| matches!(c.node, Node::File(_)));
        if let Some(i) = last_file {
            for c in &list[..i] {
                eprintln!("[mods] {}: {path} replaces the copy from {}", list[i].by, c.by);
            }
        }
        let file = |i: usize| match &list[i].node {
            Node::File(p) => p.clone(),
            Node::Archive(_) => unreachable!(),
        };
        if let Some(i) = last_file
            && i + 1 == list.len()
        {
            overlay.push((path, Arc::new(HostFile::new(file(i))?)));
            continue;
        }
        let mut data = match (last_file, originals.get(&lower)) {
            (Some(i), _) => std::fs::read(file(i))?,
            (None, Some(f)) => fs.storage().read_vec(f.offset, f.size as usize)?,
            (None, None) => {
                eprintln!("[mods] {path} is not in the game files, so {} cannot edit it", list[0].by);
                continue;
            }
        };
        let started = std::time::Instant::now();
        for c in &list[last_file.map_or(0, |i| i + 1)..] {
            let Node::Archive(edits) = &c.node else { continue };
            match edit(&data, edits) {
                Ok(new) => data = new,
                Err(e) => eprintln!("[mods] {}: cannot edit {path}: {e:#}", c.by),
            }
        }
        eprintln!("[mods] {path} edited in {:.2} s", started.elapsed().as_secs_f32());
        overlay.push((path, Arc::new(Mem(data))));
    }
    fs.with_overlay(overlay)
}
