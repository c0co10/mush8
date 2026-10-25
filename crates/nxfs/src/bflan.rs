// Mush8 - bflan.rs
// Constantin Clerc (c22dev) - 0.1
use crate::bflyt::{Axis, PaneEdit};
use anyhow::{Context, Result, ensure};

const HERMITE: u8 = 2;
const KEY_SIZE: usize = 12;

struct Curve {
    pane: String,
    target: u8,
    keys: usize,
    count: usize,
}

pub struct Animation<'a> {
    data: &'a mut [u8],
    curves: Vec<Curve>,
}

fn u16_at(d: &[u8], o: usize) -> Result<usize> {
    Ok(u16::from_le_bytes(d.get(o..o + 2).context("BFLAN truncated")?.try_into()?) as usize)
}

fn u32_at(d: &[u8], o: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(d.get(o..o + 4).context("BFLAN truncated")?.try_into()?) as usize)
}

impl<'a> Animation<'a> {
    pub fn parse(data: &'a mut [u8]) -> Result<Self> {
        ensure!(data.len() >= 0x14 && &data[..4] == b"FLAN", "not a BFLAN");
        ensure!(data[4..6] == [0xFF, 0xFE], "big-endian BFLAN");
        let d = &*data;
        let mut curves = Vec::new();
        let mut o = u16_at(d, 6)?;
        while o + 8 <= d.len() {
            let size = u32_at(d, o + 4)?;
            ensure!(size >= 8 && o + size <= d.len(), "BFLAN section at {o:#x} out of range");
            if &d[o..o + 4] == b"pai1" {
                let table = o + u32_at(d, o + 0x10)?;
                for k in 0..u16_at(d, o + 0xE)? {
                    let e = o + u32_at(d, table + 4 * k)?;
                    let name = d.get(e..e + 28).context("BFLAN truncated")?;
                    let pane = String::from_utf8_lossy(&name[..name.iter().position(|&c| c == 0).unwrap_or(28)]).into_owned();
                    for t in 0..d[e + 28] as usize {
                        let tag = e + u32_at(d, e + 0x20 + 4 * t)?;
                        if d.get(tag..tag + 4) != Some(b"FLPA") {
                            continue;
                        }
                        for c in 0..d[tag + 4] as usize {
                            let te = tag + u32_at(d, tag + 8 + 4 * c)?;
                            let (target, kind, count) = (d[te + 1], d[te + 2], u16_at(d, te + 4)?);
                            let keys = te + u32_at(d, te + 8)?;
                            ensure!(kind == HERMITE, "{pane}: unexpected curve type {kind}");
                            ensure!(keys + count * KEY_SIZE <= d.len(), "{pane}: keys out of range");
                            curves.push(Curve { pane: pane.clone(), target, keys, count });
                        }
                    }
                }
            }
            o += size;
        }
        Ok(Self { data, curves })
    }

    fn update(&mut self, pane: &str, target: u8, value: impl Fn(f32) -> f32, slope: impl Fn(f32) -> f32) -> bool {
        let mut found = false;
        for c in self.curves.iter().filter(|c| c.pane == pane && c.target == target) {
            for k in 0..c.count {
                for (field, f) in [(4, &value as &dyn Fn(f32) -> f32), (8, &slope)] {
                    let at = c.keys + k * KEY_SIZE + field;
                    let v = f(f32::from_le_bytes(self.data[at..at + 4].try_into().unwrap()));
                    self.data[at..at + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            found = true;
        }
        found
    }
}

impl PaneEdit for Animation<'_> {
    fn scale(&mut self, pane: &str, axis: Axis, by: f32) -> bool {
        self.update(pane, if axis == Axis::X { 6 } else { 7 }, |v| v * by, |v| v * by)
    }

    fn translate(&mut self, pane: &str, axis: Axis, by: f32) -> bool {
        self.update(pane, if axis == Axis::X { 0 } else { 1 }, |v| v * by, |v| v * by)
    }

    fn shift(&mut self, pane: &str, axis: Axis, by: f32) -> bool {
        self.update(pane, if axis == Axis::X { 0 } else { 1 }, |v| v + by, |v| v)
    }

    fn resize(&mut self, pane: &str, axis: Axis, by: f32) -> bool {
        self.update(pane, if axis == Axis::X { 8 } else { 9 }, |v| v * by, |v| v * by)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_pane_curves() {
        let mut d = b"FLAN\xFF\xFE\x14\x00".to_vec();
        d.resize(0x14, 0);
        let pai = d.len();
        d.extend(b"pai1\0\0\0\0\x01\0\0\0\0\0\x01\0\x14\0\0\0\x18\0\0\0");
        let entry = d.len();
        let mut name = b"N_All_00".to_vec();
        name.resize(28, 0);
        d.extend(name);
        d.extend([1, 0, 0, 0, 0x24, 0, 0, 0]);
        let tag = d.len();
        d.extend(b"FLPA\x02\0\0\0\x10\0\0\0\x1C\0\0\0");
        for target in [6u8, 0] {
            d.extend([0, target, HERMITE, 0, 1, 0, 0, 0, 0x18, 0, 0, 0]);
        }
        for v in [0.0f32, 1.05, 0.5, 0.0, -528.0, 0.0] {
            d.extend(v.to_le_bytes());
        }
        assert_eq!((entry - pai, tag - entry), (0x18, 0x24));
        let size = (d.len() - pai) as u32;
        d[pai + 4..pai + 8].copy_from_slice(&size.to_le_bytes());
        let mut a = Animation::parse(&mut d).unwrap();
        assert!(a.scale("N_All_00", Axis::X, 2.0));
        assert!(!a.scale("N_All_00", Axis::Y, 2.0));
        assert!(a.translate("N_All_00", Axis::X, 0.5));
        assert!(a.shift("N_All_00", Axis::X, 4.0));
        let f = |i: usize| f32::from_le_bytes(d[d.len() - 24 + i * 4..][..4].try_into().unwrap());
        assert_eq!([f(1), f(2), f(4), f(5)], [2.1, 1.0, -260.0, 0.0]);
    }
}
