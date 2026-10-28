// Mush8 - webm.rs
// Constantin Clerc (c22dev) - 0.1
use std::sync::Arc;

pub struct Frame {
    pub us: i64,
    pub key: bool,
    pub range: std::ops::Range<usize>,
}

pub struct Movie {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub frames: Vec<Frame>,
    pub duration_us: i64,
}

pub fn is_webm(data: &[u8]) -> bool {
    data.starts_with(&[0x1a, 0x45, 0xdf, 0xa3])
}

fn vint(d: &[u8], p: usize, mask: bool) -> Option<(u64, usize)> {
    let b = *d.get(p)?;
    let len = b.leading_zeros() as usize + 1;
    if len > 8 {
        return None;
    }
    let mut v = if mask { (b as u64) & ((1u64 << (8 - len)) - 1) } else { b as u64 };
    for i in 1..len {
        v = v << 8 | *d.get(p + i)? as u64;
    }
    Some((v, len))
}

fn uint(d: &[u8]) -> u64 {
    d.iter().fold(0, |v, &b| v << 8 | b as u64)
}

const MASTERS: [u64; 6] = [0x18538067, 0x1F43B675, 0x1654AE6B, 0xAE, 0xE0, 0xA0];

impl Movie {
    pub fn parse(data: Vec<u8>) -> Option<Arc<Self>> {
        let mut m = Movie { data: Vec::new(), width: 0, height: 0, frames: Vec::new(), duration_us: 0 };
        let mut scale_ns = 1_000_000u64;
        let mut cluster = 0u64;
        let mut stack = vec![(0usize, data.len())];
        while let Some((mut p, end)) = stack.pop() {
            while p < end {
                let (id, l) = vint(&data, p, false)?;
                let (mut size, l2) = vint(&data, p + l, true)?;
                p += l + l2;
                if size == (1u64 << (7 * l2)) - 1 {
                    size = (end - p) as u64;
                }
                let body = p..(p + size as usize).min(end);
                p = body.end;
                match id {
                    _ if MASTERS.contains(&id) => {
                        stack.push((p, end));
                        stack.push((body.start, body.end));
                        break;
                    }
                    0x2AD7B1 => scale_ns = uint(&data[body]),
                    0xB0 => m.width = uint(&data[body]) as u32,
                    0xBA => m.height = uint(&data[body]) as u32,
                    0xE7 => cluster = uint(&data[body]),
                    0xA3 | 0xA1 => {
                        let (_, tl) = vint(&data, body.start, true)?;
                        let h = body.start + tl;
                        let rel = i16::from_be_bytes([*data.get(h)?, *data.get(h + 1)?]) as i64;
                        let flags = *data.get(h + 2)?;
                        let us = (cluster as i64 + rel) * scale_ns as i64 / 1000;
                        let key = id == 0xA3 && flags & 0x80 != 0 || id == 0xA1 && m.frames.is_empty();
                        m.frames.push(Frame { us, key, range: h + 3..body.end });
                    }
                    _ => {}
                }
            }
        }
        m.frames.sort_by_key(|f| f.us);
        let step = m.frames.windows(2).last().map_or(0, |w| w[1].us - w[0].us);
        m.duration_us = m.frames.last().map_or(0, |f| f.us + step);
        m.data = data;
        (m.width > 0 && m.height > 0 && !m.frames.is_empty()).then(|| Arc::new(m))
    }
}
