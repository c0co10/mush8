// Mush8 - romfs.rs
// Constantin Clerc (c22dev) - 0.1
use crate::storage::{DynStorage, Patched};
use anyhow::{Context, Result, ensure};
use std::sync::Arc;

const NONE: u32 = u32::MAX;

pub struct File {
    pub path: String,
    pub offset: u64,
    pub size: u64,
    entry: usize,
}

pub struct RomFs {
    storage: DynStorage,
    data_offset: u64,
    files_offset: u64,
    dirs: Vec<u8>,
    files: Vec<u8>,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

impl RomFs {
    pub fn open(storage: DynStorage) -> Result<Self> {
        let h = storage.read_vec(0, 0x50)?;
        ensure!(u64_at(&h, 0) == 0x50, "bad RomFS header size");
        let dirs = storage.read_vec(u64_at(&h, 0x18), u64_at(&h, 0x20) as usize)?;
        let files = storage.read_vec(u64_at(&h, 0x38), u64_at(&h, 0x40) as usize)?;
        Ok(Self { storage, data_offset: u64_at(&h, 0x48), files_offset: u64_at(&h, 0x38), dirs, files })
    }

    pub fn storage(&self) -> &DynStorage {
        &self.storage
    }

    pub fn files(&self) -> Vec<File> {
        let mut out = Vec::new();
        self.walk(0, String::new(), &mut out);
        out
    }

    pub fn with_files(&self, replacements: Vec<(String, Vec<u8>)>) -> Result<DynStorage> {
        let files = self.files();
        let mut table = self.files.clone();
        let mut end = self.storage.size();
        let mut patches = Vec::new();
        for (path, data) in replacements {
            let f = files.iter().find(|f| f.path == path).with_context(|| format!("{path} is not in the RomFS"))?;
            end = end.next_multiple_of(0x10);
            table[f.entry + 8..f.entry + 0x10].copy_from_slice(&(end - self.data_offset).to_le_bytes());
            table[f.entry + 0x10..f.entry + 0x18].copy_from_slice(&(data.len() as u64).to_le_bytes());
            let len = data.len() as u64;
            patches.push((end, Arc::<[u8]>::from(data)));
            end += len;
        }
        patches.push((self.files_offset, table.into()));
        Ok(Arc::new(Patched::new(self.storage.clone(), end, patches)))
    }

    fn walk(&self, dir: u32, path: String, out: &mut Vec<File>) {
        let d = &self.dirs[dir as usize..];
        let mut f = u32_at(d, 0xC);
        while f != NONE {
            let e = &self.files[f as usize..];
            let len = u32_at(e, 0x1C) as usize;
            let name = String::from_utf8_lossy(&e[0x20..0x20 + len]);
            out.push(File { path: format!("{path}/{name}"), offset: self.data_offset + u64_at(e, 8), size: u64_at(e, 0x10), entry: f as usize });
            f = u32_at(e, 4);
        }
        let mut c = u32_at(d, 8);
        while c != NONE {
            let e = &self.dirs[c as usize..];
            let len = u32_at(e, 0x14) as usize;
            let name = String::from_utf8_lossy(&e[0x18..0x18 + len]);
            self.walk(c, format!("{path}/{name}"), out);
            c = u32_at(e, 4);
        }
    }
}
