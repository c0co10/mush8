// Mush8 - nsd.rs
// Constantin Clerc (c22dev) - 0.1
use crate::ipc::{Request, Response, Service, Stub, write_buffer};
use crate::services::bsd::{bytes, in_buf, out_buf};
use crate::services::nextendo;

const ENV_LP: u8 = 1;

pub struct Nsd;

impl Service for Nsd {
    fn name(&self) -> String {
        "nsd".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            11 => {
                if let Some(b) = out_buf(req, 0) {
                    write_buffer(&b, &[b'l', b'p', b'1', 0, 0, 0, 0, 0]);
                }
            }
            20 | 21 => {
                let raw = bytes(in_buf(req, 0));
                let host = String::from_utf8_lossy(&raw[..raw.iter().position(|&c| c == 0).unwrap_or(raw.len())]).into_owned();
                if let Some(b) = out_buf(req, 0) {
                    let mut out = vec![0u8; b.size as usize];
                    let resolved = nextendo::nsd_resolve(&host);
                    let n = resolved.len().min(out.len().saturating_sub(1));
                    out[..n].copy_from_slice(&resolved.as_bytes()[..n]);
                    write_buffer(&b, &out);
                }
                if req.cmd == 21 {
                    rsp.push_u32(0);
                }
            }
            63 => rsp.data.push(0),
            100 => rsp.data.push(ENV_LP),
            12 | 13 | 14 | 15 | 101 | 102 => {}
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}
