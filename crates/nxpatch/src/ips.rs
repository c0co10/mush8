use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};

const NSO_HEADER: u64 = 0x100;

pub struct CodePatch {
    pub source: PathBuf,
    pub build_id: String,
    pub writes: Vec<(u64, Vec<u8>)>,
}

fn normalize(id: &str) -> String {
    id.trim().to_ascii_lowercase().trim_end_matches('0').to_string()
}

pub fn load(path: &Path) -> Result<CodePatch> {
    let data = std::fs::read(path).with_context(|| path.display().to_string())?;
    let is_text = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("pchtxt"));
    let (build_id, writes) = if is_text {
        pchtxt(&String::from_utf8_lossy(&data))?
    } else {
        let stem = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.split('.').next()).unwrap_or_default();
        (stem.to_string(), ips(&data)?)
    };
    let build_id = normalize(&build_id);
    ensure!(!build_id.is_empty() && build_id.bytes().all(|b| b.is_ascii_hexdigit()), "{}: no build id", path.display());
    Ok(CodePatch { source: path.to_path_buf(), build_id, writes })
}

impl CodePatch {
    pub fn matches(&self, module_id: &[u8]) -> bool {
        normalize(&hex::encode(module_id)) == self.build_id
    }

    pub fn apply(&self, image: &mut [u8]) -> Result<usize> {
        let mut bytes = 0;
        for (offset, data) in &self.writes {
            let Some(at) = offset.checked_sub(NSO_HEADER) else { continue };
            let at = at as usize;
            let end = at + data.len();
            ensure!(end <= image.len(), "{}: write at {:#x} is past the end of the module", self.source.display(), at);
            image[at..end].copy_from_slice(data);
            bytes += data.len();
        }
        Ok(bytes)
    }
}

fn be(b: &[u8]) -> u64 {
    b.iter().fold(0, |v, &x| v << 8 | x as u64)
}

pub fn ips(data: &[u8]) -> Result<Vec<(u64, Vec<u8>)>> {
    let (width, end_marker): (usize, &[u8]) = if data.starts_with(b"IPS32") {
        (4, b"EEOF")
    } else if data.starts_with(b"PATCH") {
        (3, b"EOF")
    } else {
        bail!("not an IPS patch");
    };
    let mut p = 5;
    let mut out = Vec::new();
    loop {
        let rest = &data[p..];
        if rest.starts_with(end_marker) && rest.len() < width + 2 + end_marker.len() {
            return Ok(out);
        }
        ensure!(rest.len() >= width + 2, "IPS patch ends without {}", String::from_utf8_lossy(end_marker));
        let offset = be(&rest[..width]);
        let size = be(&rest[width..width + 2]) as usize;
        p += width + 2;
        if size == 0 {
            ensure!(data.len() >= p + 3, "IPS run record cut short");
            let count = be(&data[p..p + 2]) as usize;
            out.push((offset, vec![data[p + 2]; count]));
            p += 3;
        } else {
            ensure!(data.len() >= p + size, "IPS record cut short");
            out.push((offset, data[p..p + size].to_vec()));
            p += size;
        }
    }
}

fn parse_num(s: &str) -> Result<u64> {
    let s = s.trim();
    Ok(match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16)?,
        None => s.parse()?,
    })
}

fn quoted(s: &str) -> Result<Vec<u8>> {
    let inner = s.strip_prefix('"').and_then(|s| s.rfind('"').map(|e| &s[..e])).context("unterminated string")?;
    let mut out = Vec::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        let c = if c == '\\' {
            match chars.next() {
                Some('n') => '\n',
                Some('t') => '\t',
                Some('r') => '\r',
                Some('0') => '\0',
                Some(c) => c,
                None => '\\',
            }
        } else {
            c
        };
        let mut buf = [0; 4];
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    }
    Ok(out)
}

pub fn pchtxt(text: &str) -> Result<(String, Vec<(u64, Vec<u8>)>)> {
    let mut build_id = String::new();
    let mut shift = 0u64;
    let mut little_endian = false;
    let mut enabled = false;
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if let Some(id) = line.strip_prefix("@nsobid-") {
            build_id = id.trim().to_string();
        } else if line.starts_with("@stop") {
            break;
        } else if line.starts_with("@enabled") {
            enabled = true;
        } else if line.starts_with("@disabled") {
            enabled = false;
        } else if line.starts_with("@little-endian") {
            little_endian = true;
        } else if line.starts_with("@big-endian") {
            little_endian = false;
        } else if let Some(flag) = line.strip_prefix("@flag") {
            let mut words = flag.split_whitespace();
            if words.next() == Some("offset_shift") {
                shift = parse_num(words.next().context("offset_shift without a value")?).with_context(|| format!("line {}", n + 1))?;
            }
        } else if enabled && !line.is_empty() && !line.starts_with('@') && !line.starts_with('#') && !line.starts_with("//") {
            let (offset, value) = line.split_once(char::is_whitespace).with_context(|| format!("line {}: no value", n + 1))?;
            let offset = u64::from_str_radix(offset, 16).with_context(|| format!("line {}: bad offset", n + 1))?;
            let value = value.trim();
            let bytes = if value.starts_with('"') {
                quoted(value)
            } else {
                let hex = value.split("//").next().unwrap_or_default().trim();
                let mut b = hex::decode(hex).map_err(anyhow::Error::from)?;
                if little_endian {
                    b.reverse();
                }
                Ok(b)
            }
            .with_context(|| format!("line {}", n + 1))?;
            out.push((offset + shift, bytes));
        }
    }
    Ok((build_id, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ips_records_and_runs() {
        let mut p = b"PATCH".to_vec();
        p.extend_from_slice(&[0x00, 0x01, 0x04, 0x00, 0x02, 0xAA, 0xBB]);
        p.extend_from_slice(&[0x00, 0x01, 0x10, 0x00, 0x00, 0x00, 0x03, 0xCC]);
        p.extend_from_slice(b"EOF");
        assert_eq!(ips(&p).unwrap(), vec![(0x104, vec![0xAA, 0xBB]), (0x110, vec![0xCC; 3])]);
    }

    #[test]
    fn ips_record_at_eof_offset() {
        let mut p = b"PATCH".to_vec();
        p.extend_from_slice(b"EOF");
        p.extend_from_slice(&[0x00, 0x01, 0x7F]);
        p.extend_from_slice(b"EOF");
        assert_eq!(ips(&p).unwrap(), vec![(0x454F46, vec![0x7F])]);
    }

    #[test]
    fn ips32_offsets() {
        let mut p = b"IPS32".to_vec();
        p.extend_from_slice(&[0x01, 0x00, 0x01, 0x00, 0x00, 0x01, 0x55]);
        p.extend_from_slice(b"EEOF");
        assert_eq!(ips(&p).unwrap(), vec![(0x0100_0100, vec![0x55])]);
    }

    #[test]
    fn pchtxt_blocks_shift_and_comments() {
        let text = "@nsobid-158C0C7399AABD7BF83795C2EF3484C7\n# title\n@flag offset_shift 0x100\n@disabled\n00000010 FFFFFFFF\n@enabled\n// note\n0061BFA4 1044532D\n00000020 \"hi\"\n@little-endian\n00000030 0102 // tail\n@stop\n00000040 FF\n";
        let (id, writes) = pchtxt(text).unwrap();
        assert_eq!(id, "158C0C7399AABD7BF83795C2EF3484C7");
        assert_eq!(writes, vec![(0x61C0A4, vec![0x10, 0x44, 0x53, 0x2D]), (0x120, b"hi".to_vec()), (0x130, vec![0x02, 0x01])]);
    }

    #[test]
    fn build_id_ignores_case_and_trailing_zeros() {
        let p = CodePatch { source: PathBuf::new(), build_id: normalize("FE941ED5BA14BE5D505698DA1BBF4FE7"), writes: vec![] };
        let mut id = [0u8; 32];
        id[..16].copy_from_slice(&hex::decode("fe941ed5ba14be5d505698da1bbf4fe7").unwrap());
        assert!(p.matches(&id));
        id[0] = 0;
        assert!(!p.matches(&id));
    }

    #[test]
    fn apply_subtracts_the_nso_header() {
        let p = CodePatch { source: PathBuf::new(), build_id: String::new(), writes: vec![(0x104, vec![1, 2]), (0x10, vec![9])] };
        let mut image = vec![0u8; 8];
        assert_eq!(p.apply(&mut image).unwrap(), 2);
        assert_eq!(image, vec![0, 0, 0, 0, 1, 2, 0, 0]);
    }
}
