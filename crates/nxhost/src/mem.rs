// Mush8 - mem.rs
// Constantin Clerc (c22dev) - 0.1
use anyhow::{Result, ensure};
use std::collections::BTreeMap;
use std::sync::Mutex;

pub const PAGE: u64 = 0x4000;
pub const ADDRESS_SPACE_END: u64 = 1 << 39;

pub const CODE_OFF: u64 = 0;
pub const TLR_OFF: u64 = 0x0F00_0000;
pub const HEAP_OFF: u64 = 0x1000_0000;

#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub heap_size: u64,
    pub alias_size: u64,
    pub stack_size: u64,
}

impl Layout {
    pub fn alias_off(&self) -> u64 {
        HEAP_OFF + self.heap_size
    }
    pub fn stack_off(&self) -> u64 {
        self.alias_off() + self.alias_size
    }
    pub fn total(&self) -> u64 {
        self.stack_off() + self.stack_size
    }
}

const FULL: Layout = Layout { heap_size: 0x1_0000_0000, alias_size: 0x1_0000_0000, stack_size: 0x7000_0000 };
const COMPACT: Layout = Layout { heap_size: 0xD000_0000, alias_size: 0x4000_0000, stack_size: 0x2000_0000 };

pub mod state {
    pub const FREE: u32 = 0x00;
    pub const CODE: u32 = 0x03;
    pub const CODE_DATA: u32 = 0x04;
    pub const NORMAL: u32 = 0x05;
    pub const SHARED: u32 = 0x06;
    pub const ALIAS: u32 = 0x07;
    pub const STACK: u32 = 0x0B;
    pub const THREAD_LOCAL: u32 = 0x0C;
    pub const INACCESSIBLE: u32 = 0x10;
}

pub mod perm {
    pub const NONE: u32 = 0;
    pub const R: u32 = 1;
    pub const W: u32 = 2;
    pub const X: u32 = 4;
    pub const RW: u32 = R | W;
    pub const RX: u32 = R | X;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub state: u32,
    pub perm: u32,
    pub attr: u32,
}

pub struct AddressSpace {
    pub base: u64,
    pub layout: Layout,
    pub map: Mutex<MemMap>,
}

impl AddressSpace {
    pub fn reserve() -> Result<Self> {
        unsafe extern "C" {
            fn mach_vm_allocate(task: u32, addr: *mut u64, size: u64, flags: i32) -> i32;
            static mach_task_self_: u32;
        }
        let task = unsafe { mach_task_self_ };
        const STEP: u64 = 0x4000_0000;
        let find = |total: u64| {
            (1..ADDRESS_SPACE_END / STEP).rev().map(|i| i * STEP).filter(|&cand| cand + total <= ADDRESS_SPACE_END).find(|&cand| {
                let mut a = cand;
                unsafe { mach_vm_allocate(task, &mut a, total, 0) == 0 }
            })
        };
        let layouts = if std::env::var_os("NX_COMPACT_MEMORY").is_some() { vec![COMPACT] } else { vec![FULL, COMPACT] };
        let (base, layout) = layouts
            .into_iter()
            .find_map(|l| find(l.total()).map(|b| (b, l)))
            .ok_or_else(|| anyhow::anyhow!("no free {:#x}-byte range below 2^39", COMPACT.total()))?;
        if layout.total() != FULL.total() {
            eprintln!("[host] small address space: compact guest layout ({:#x} bytes)", layout.total());
        }
        protect(base, layout.total(), perm::NONE)?;
        let mut map = MemMap::default();
        map.set(0, ADDRESS_SPACE_END, Block { state: state::FREE, perm: 0, attr: 0 });
        map.set(base, base + layout.total(), Block { state: state::FREE, perm: 0, attr: 0 });
        Ok(Self { base, layout, map: Mutex::new(map) })
    }

    pub fn code(&self) -> u64 {
        self.base + CODE_OFF
    }
    pub fn heap(&self) -> u64 {
        self.base + HEAP_OFF
    }
    pub fn alias(&self) -> u64 {
        self.base + self.layout.alias_off()
    }
    pub fn stack(&self) -> u64 {
        self.base + self.layout.stack_off()
    }

    pub fn readable(&self, addr: u64, len: u64) -> bool {
        let (start, size, b) = self.map.lock().unwrap().query(addr);
        b.perm & perm::R != 0 && addr.checked_add(len).is_some_and(|end| end <= start + size)
    }

    pub fn set(&self, addr: u64, size: u64, b: Block) -> Result<()> {
        let (lo, hi) = (addr & !(PAGE - 1), (addr + size).next_multiple_of(PAGE));
        let (inner_lo, inner_hi) = (addr.next_multiple_of(PAGE), (addr + size) & !(PAGE - 1));
        if inner_lo < inner_hi {
            protect(inner_lo, inner_hi - inner_lo, b.perm)?;
            if lo < inner_lo {
                protect(lo, inner_lo - lo, b.perm | perm::RW)?;
            }
            if inner_hi < hi {
                protect(inner_hi, hi - inner_hi, b.perm | perm::RW)?;
            }
        } else {
            protect(lo, hi - lo, b.perm | perm::RW)?;
        }
        self.map.lock().unwrap().set(addr, addr + size, b);
        Ok(())
    }

    pub fn discard(&self, addr: u64, size: u64) -> Result<()> {
        let (inner_lo, inner_hi) = (addr.next_multiple_of(PAGE), (addr + size) & !(PAGE - 1));
        if inner_lo < inner_hi {
            let p = unsafe {
                libc::mmap(inner_lo as *mut _, (inner_hi - inner_lo) as usize, libc::PROT_NONE, libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_FIXED, -1, 0)
            };
            ensure!(p as u64 == inner_lo, "discarding {inner_lo:#x}+{:#x} failed", inner_hi - inner_lo);
            for (a, b) in [(addr, inner_lo), (inner_hi, addr + size)] {
                if a < b {
                    unsafe { std::ptr::write_bytes(a as *mut u8, 0, (b - a) as usize) };
                }
            }
        } else {
            unsafe { std::ptr::write_bytes(addr as *mut u8, 0, size as usize) };
        }
        self.map.lock().unwrap().set(addr, addr + size, Block { state: state::FREE, perm: 0, attr: 0 });
        Ok(())
    }
}

pub fn protect(addr: u64, size: u64, perm: u32) -> Result<()> {
    let mut prot = 0;
    if perm & perm::R != 0 {
        prot |= libc::PROT_READ;
    }
    if perm & perm::W != 0 {
        prot |= libc::PROT_WRITE;
    }
    if perm & perm::X != 0 {
        prot |= libc::PROT_EXEC;
    }
    let r = unsafe { libc::mprotect(addr as *mut _, size as usize, prot) };
    ensure!(r == 0, "mprotect({addr:#x}, {size:#x}, {perm}) failed: {}", std::io::Error::last_os_error());
    Ok(())
}

#[derive(Default)]
pub struct MemMap {
    blocks: BTreeMap<u64, Block>,
}

impl MemMap {
    pub fn set(&mut self, start: u64, end: u64, b: Block) {
        if start >= end {
            return;
        }
        let after = self.get(end);
        let keys: Vec<u64> = self.blocks.range(start..end).map(|(&k, _)| k).collect();
        for k in keys {
            self.blocks.remove(&k);
        }
        self.blocks.insert(start, b);
        if end < ADDRESS_SPACE_END {
            self.blocks.entry(end).or_insert(after.unwrap_or(b));
        }
    }

    fn get(&self, addr: u64) -> Option<Block> {
        self.blocks.range(..=addr).next_back().map(|(_, &b)| b)
    }

    pub fn query(&self, addr: u64) -> (u64, u64, Block) {
        if addr >= ADDRESS_SPACE_END {
            return (ADDRESS_SPACE_END, u64::MAX - ADDRESS_SPACE_END + 1, Block { state: state::INACCESSIBLE, perm: 0, attr: 0 });
        }
        let (&k, &b) = self.blocks.range(..=addr).next_back().expect("map covers the address space");
        let mut start = k;
        for (&pk, &pb) in self.blocks.range(..k).rev() {
            if pb != b {
                break;
            }
            start = pk;
        }
        let mut end = ADDRESS_SPACE_END;
        for (&nk, &nb) in self.blocks.range(k + 1..) {
            if nb != b {
                end = nk;
                break;
            }
        }
        (start, end - start, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memmap_split_and_merge() {
        let mut m = MemMap::default();
        let free = Block { state: state::FREE, perm: 0, attr: 0 };
        let heap = Block { state: state::NORMAL, perm: perm::RW, attr: 0 };
        m.set(0, ADDRESS_SPACE_END, free);
        m.set(0x1000, 0x3000, heap);
        assert_eq!(m.query(0x2000), (0x1000, 0x2000, heap));
        assert_eq!(m.query(0x0), (0, 0x1000, free));
        assert_eq!(m.query(0x3000), (0x3000, ADDRESS_SPACE_END - 0x3000, free));
        m.set(0x3000, 0x4000, heap);
        assert_eq!(m.query(0x1000), (0x1000, 0x3000, heap));
        m.set(0x2000, 0x2800, free);
        assert_eq!(m.query(0x1000), (0x1000, 0x1000, heap));
        assert_eq!(m.query(0x2400), (0x2000, 0x800, free));
        assert_eq!(m.query(0x2800), (0x2800, 0x1800, heap));
    }
}
