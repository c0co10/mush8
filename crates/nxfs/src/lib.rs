// Mush8 - lib.rs
// Constantin Clerc (c22dev) - 0.1
pub mod bflan;
pub mod bflyt;
pub mod crypto;
pub mod keys;
pub mod module;
pub mod nca;
pub mod nso;
pub mod pfs0;
pub mod romfs;
pub mod sarc;
pub mod storage;

use anyhow::{Context, Result};
use nca::{Nca, TitleKeys};
use pfs0::Pfs0;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use storage::{DynStorage, FileRange};

pub struct Nsp {
    pub ncas: Vec<Nca>,
}

impl Nsp {
    pub fn open(path: &Path, keys: &keys::Keys) -> Result<Self> {
        let f = Arc::new(File::open(path).with_context(|| format!("opening {}", path.display()))?);
        let size = f.metadata()?.len();
        let pfs = Pfs0::open(Arc::new(FileRange::new(f, 0, size)))?;

        let mut title_keys = TitleKeys::new();
        for e in pfs.entries.iter().filter(|e| e.name.ends_with(".tik")) {
            let tik = pfs.storage_of(e).read_vec(0, e.size as usize)?;
            let (rights, key) = nca::ticket_title_key(keys, &tik).with_context(|| e.name.clone())?;
            title_keys.insert(rights, key);
        }

        let mut ncas = Vec::new();
        for e in pfs.entries.iter().filter(|e| e.name.ends_with(".nca")) {
            let s: DynStorage = pfs.storage_of(e);
            ncas.push(Nca::open(&e.name, s, keys, &title_keys)?);
        }
        Ok(Self { ncas })
    }

    pub fn find(&self, t: nca::ContentType) -> Option<&Nca> {
        self.ncas.iter().find(|n| n.content_type == t)
    }
}

pub fn open_nca(path: &Path, keys: &keys::Keys) -> Result<Nca> {
    let f = Arc::new(File::open(path).with_context(|| format!("opening {}", path.display()))?);
    let size = f.metadata()?.len();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Nca::open(&name, Arc::new(FileRange::new(f, 0, size)), keys, &TitleKeys::new())
}
