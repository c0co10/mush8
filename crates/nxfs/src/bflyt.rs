// Mush8 - bflyt.rs
// Constantin Clerc (c22dev) - 0.1
use anyhow::{Result, ensure};

const PANE_KINDS: [&[u8; 4]; 7] = [b"pan1", b"pic1", b"txt1", b"wnd1", b"prt1", b"bnd1", b"ali1"];
const PANE_SIZE: usize = 0x54;
const NAME: usize = 0xC;
const TRANSLATE: usize = 0x2C;
const SCALE: usize = 0x44;
const SIZE: usize = 0x4C;
const PIC_MATERIAL: usize = 0x64;
const MAT_FLAGS: usize = 0x1C;
const MAT_LAYOUT: usize = 0x20;
const MAT_TEXMAPS: usize = 0x2C;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

pub trait PaneEdit {
    fn scale(&mut self, pane: &str, axis: Axis, by: f32) -> bool;
    fn translate(&mut self, pane: &str, axis: Axis, by: f32) -> bool;
    fn shift(&mut self, pane: &str, axis: Axis, by: f32) -> bool;
    fn resize(&mut self, pane: &str, axis: Axis, by: f32) -> bool;
    fn tile(&mut self, _pane: &str, _axis: Axis, _by: f32) -> bool {
        false
    }
}

#[derive(Clone, Debug)]
pub struct Pane {
    pub name: String,
    pub kind: [u8; 4],
    pub parent: Option<usize>,
    pub translate: [f32; 2],
    pub scale: [f32; 2],
    pub size: [f32; 2],
    pub user_data: Vec<String>,
    offset: usize,
}

pub struct Layout<'a> {
    data: &'a mut [u8],
    panes: Vec<Pane>,
    materials: Vec<usize>,
}

fn f32_at(d: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

fn cstr(d: &[u8]) -> String {
    String::from_utf8_lossy(&d[..d.iter().position(|&c| c == 0).unwrap_or(d.len())]).into_owned()
}

fn user_data_names(d: &[u8]) -> Vec<String> {
    let count = u16::from_le_bytes([d[8], d[9]]) as usize;
    (0..count)
        .filter_map(|i| {
            let entry = 0xC + i * 0xC;
            let name = entry + u32::from_le_bytes(d.get(entry..entry + 4)?.try_into().ok()?) as usize;
            Some(cstr(d.get(name..)?))
        })
        .collect()
}

fn axis_offset(axis: Axis) -> usize {
    if axis == Axis::Y { 4 } else { 0 }
}

impl<'a> Layout<'a> {
    pub fn parse(data: &'a mut [u8]) -> Result<Self> {
        ensure!(data.len() >= 0x14 && &data[..4] == b"FLYT", "not a BFLYT");
        ensure!(data[4..6] == [0xFF, 0xFE], "big-endian BFLYT");
        let mut o = u16::from_le_bytes([data[6], data[7]]) as usize;
        let mut panes: Vec<Pane> = Vec::new();
        let mut parents = Vec::new();
        let mut materials = Vec::new();
        while o + 8 <= data.len() {
            let size = u32::from_le_bytes(data[o + 4..o + 8].try_into()?) as usize;
            ensure!(size >= 8 && o + size <= data.len(), "BFLYT section at {o:#x} out of range");
            let kind: [u8; 4] = data[o..o + 4].try_into()?;
            if size >= PANE_SIZE && PANE_KINDS.contains(&&kind) {
                panes.push(Pane {
                    name: cstr(&data[o + NAME..o + NAME + 0x18]),
                    kind,
                    parent: parents.last().copied().flatten(),
                    translate: [f32_at(data, o + TRANSLATE), f32_at(data, o + TRANSLATE + 4)],
                    scale: [f32_at(data, o + SCALE), f32_at(data, o + SCALE + 4)],
                    size: [f32_at(data, o + SIZE), f32_at(data, o + SIZE + 4)],
                    user_data: Vec::new(),
                    offset: o,
                });
            } else if &kind == b"mat1" && size >= 0xC {
                let count = u16::from_le_bytes([data[o + 8], data[o + 9]]) as usize;
                materials = (0..count)
                    .filter_map(|i| Some(o + u32::from_le_bytes(data.get(o + 0xC + 4 * i..o + 0x10 + 4 * i)?.try_into().ok()?) as usize))
                    .filter(|&m| m + MAT_TEXMAPS <= data.len())
                    .collect();
            } else if &kind == b"usd1" && size >= 0xC {
                if let Some(p) = panes.last_mut() {
                    p.user_data = user_data_names(&data[o..o + size]);
                }
            } else if &kind == b"pas1" {
                parents.push(panes.len().checked_sub(1));
            } else if &kind == b"pae1" {
                parents.pop();
            }
            o += size;
        }
        Ok(Self { data, panes, materials })
    }

    pub fn panes(&self) -> &[Pane] {
        &self.panes
    }

    pub fn pane(&self, name: &str) -> Option<&Pane> {
        self.panes.iter().find(|p| p.name == name)
    }

    pub fn ancestors(&self, pane: &Pane) -> impl Iterator<Item = &Pane> {
        std::iter::successors(pane.parent.map(|i| &self.panes[i]), |p| p.parent.map(|i| &self.panes[i]))
    }

    fn update(&mut self, pane: &str, field: usize, axis: Axis, f: impl Fn(f32) -> f32) -> bool {
        let Some(p) = self.panes.iter().find(|p| p.name == pane) else { return false };
        let at = p.offset + field + axis_offset(axis);
        let v = f(f32_at(self.data, at));
        self.data[at..at + 4].copy_from_slice(&v.to_le_bytes());
        true
    }
}

impl PaneEdit for Layout<'_> {
    fn scale(&mut self, pane: &str, axis: Axis, by: f32) -> bool {
        self.update(pane, SCALE, axis, |v| v * by)
    }

    fn translate(&mut self, pane: &str, axis: Axis, by: f32) -> bool {
        self.update(pane, TRANSLATE, axis, |v| v * by)
    }

    fn shift(&mut self, pane: &str, axis: Axis, by: f32) -> bool {
        self.update(pane, TRANSLATE, axis, |v| v + by)
    }

    fn resize(&mut self, pane: &str, axis: Axis, by: f32) -> bool {
        self.update(pane, SIZE, axis, |v| v * by)
    }

    fn tile(&mut self, pane: &str, axis: Axis, by: f32) -> bool {
        let Some(p) = self.panes.iter().find(|p| p.name == pane && &p.kind == b"pic1") else { return false };
        let index = u16::from_le_bytes([self.data[p.offset + PIC_MATERIAL], self.data[p.offset + PIC_MATERIAL + 1]]) as usize;
        let Some(&m) = self.materials.get(index) else { return false };
        if self.data.get(m + MAT_LAYOUT..m + MAT_LAYOUT + 4) != Some(&[0, 2, 4, 8][..]) {
            return false;
        }
        let flags = u32::from_le_bytes(self.data[m + MAT_FLAGS..m + MAT_FLAGS + 4].try_into().unwrap());
        let srt = m + MAT_TEXMAPS + 4 * (flags & 3) as usize;
        let count = (flags >> 2 & 3) as usize;
        for k in 0..count {
            let at = srt + k * 0x14 + 0xC + axis_offset(axis);
            let v = f32_at(self.data, at) * by;
            self.data[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        count > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(kind: &[u8; 4], name: &str, translate: [f32; 2], scale: [f32; 2]) -> Vec<u8> {
        let mut p = vec![0u8; PANE_SIZE];
        p[..4].copy_from_slice(kind);
        p[4..8].copy_from_slice(&(PANE_SIZE as u32).to_le_bytes());
        p[NAME..NAME + name.len()].copy_from_slice(name.as_bytes());
        for (i, v) in translate.into_iter().chain(scale).enumerate() {
            let at = if i < 2 { TRANSLATE + i * 4 } else { SCALE + (i - 2) * 4 };
            p[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        p
    }

    #[test]
    fn edits_named_panes() {
        let mut d = b"FLYT\xFF\xFE\x14\x00".to_vec();
        d.resize(0x14, 0);
        d.extend(b"lyt1\x10\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00");
        d.extend(pane(b"pan1", "RootPane", [0.0, 0.0], [1.0, 1.0]));
        d.extend(b"pas1\x08\x00\x00\x00");
        d.extend(pane(b"prt1", "L_ItemBox_00", [-528.0, 261.0], [0.9, 0.9]));
        d.extend(b"usd1\x28\0\0\0\x01\0\0\0\x0C\0\0\0\0\0\0\0\0\0\0\0FrameBufferUse\0\0");
        d.extend(b"pae1\x08\x00\x00\x00");
        let mut l = Layout::parse(&mut d).unwrap();
        assert_eq!(l.pane("L_ItemBox_00").unwrap().user_data, ["FrameBufferUse"]);
        assert_eq!(l.panes().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["RootPane", "L_ItemBox_00"]);
        let item = l.pane("L_ItemBox_00").unwrap();
        assert_eq!((item.translate, item.scale, item.size), ([-528.0, 261.0], [0.9, 0.9], [0.0, 0.0]));
        assert_eq!(l.ancestors(item).map(|p| p.name.as_str()).collect::<Vec<_>>(), ["RootPane"]);
        assert!(l.scale("RootPane", Axis::X, 0.5));
        assert!(l.translate("L_ItemBox_00", Axis::X, 2.0));
        assert!(l.shift("L_ItemBox_00", Axis::X, 6.0));
        assert!(l.resize("L_ItemBox_00", Axis::Y, 2.0));
        assert!(!l.scale("N_Missing_00", Axis::Y, 2.0));
        let f = |o: usize| f32::from_le_bytes(d[o..o + 4].try_into().unwrap());
        let root = 0x14 + 0x10;
        let item = root + PANE_SIZE + 8;
        assert_eq!((f(root + SCALE), f(root + SCALE + 4)), (0.5, 1.0));
        assert_eq!((f(item + TRANSLATE), f(item + TRANSLATE + 4)), (-1050.0, 261.0));
    }
}
