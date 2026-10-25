// Mush8 - storage.rs
// Constantin Clerc (c22dev) - 0.1
use anyhow::{Result, bail};
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::sync::Arc;

pub trait Storage: Send + Sync {
    fn size(&self) -> u64;
    fn read_at(&self, off: u64, buf: &mut [u8]) -> Result<()>;

    fn read_vec(&self, off: u64, len: usize) -> Result<Vec<u8>> {
        let mut v = vec![0; len];
        self.read_at(off, &mut v)?;
        Ok(v)
    }
}

pub type DynStorage = Arc<dyn Storage>;

pub struct FileRange {
    file: Arc<File>,
    base: u64,
    size: u64,
}

impl FileRange {
    pub fn new(file: Arc<File>, base: u64, size: u64) -> Self {
        Self { file, base, size }
    }
}

impl Storage for FileRange {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_at(&self, off: u64, buf: &mut [u8]) -> Result<()> {
        if off + buf.len() as u64 > self.size {
            bail!("read past end of file range ({off:#x}+{:#x} > {:#x})", buf.len(), self.size);
        }
        self.file.read_exact_at(buf, self.base + off)?;
        Ok(())
    }
}

pub struct Sub {
    inner: DynStorage,
    base: u64,
    size: u64,
}

impl Sub {
    pub fn new(inner: DynStorage, base: u64, size: u64) -> Self {
        Self { inner, base, size }
    }
}

impl Storage for Sub {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_at(&self, off: u64, buf: &mut [u8]) -> Result<()> {
        if off + buf.len() as u64 > self.size {
            bail!("read past end of sub storage");
        }
        self.inner.read_at(self.base + off, buf)
    }
}

pub struct Mem(pub Vec<u8>);

impl Storage for Mem {
    fn size(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, off: u64, buf: &mut [u8]) -> Result<()> {
        let off = off as usize;
        if off + buf.len() > self.0.len() {
            bail!("read past end of memory storage");
        }
        buf.copy_from_slice(&self.0[off..off + buf.len()]);
        Ok(())
    }
}

pub struct Patched {
    base: DynStorage,
    size: u64,
    patches: Vec<(u64, Arc<[u8]>)>,
}

impl Patched {
    pub fn new(base: DynStorage, size: u64, patches: Vec<(u64, Arc<[u8]>)>) -> Self {
        Self { base, size, patches }
    }
}

impl Storage for Patched {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_at(&self, off: u64, buf: &mut [u8]) -> Result<()> {
        let end = off + buf.len() as u64;
        if end > self.size {
            bail!("read past end of patched storage");
        }
        let from_base = self.base.size().saturating_sub(off).min(buf.len() as u64) as usize;
        if from_base > 0 {
            self.base.read_at(off, &mut buf[..from_base])?;
        }
        buf[from_base..].fill(0);
        for (at, data) in &self.patches {
            let (lo, hi) = (off.max(*at), end.min(at + data.len() as u64));
            if lo < hi {
                buf[(lo - off) as usize..(hi - off) as usize].copy_from_slice(&data[(lo - at) as usize..(hi - at) as usize]);
            }
        }
        Ok(())
    }
}
