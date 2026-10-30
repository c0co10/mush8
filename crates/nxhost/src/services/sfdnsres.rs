// Mush8 - sfdnsres.rs
// Constantin Clerc (c22dev) - 0.1
use crate::ipc::{Request, Response, Service, Stub, write_buffer};
use crate::services::bsd::{bytes, in_buf, out_buf};
use crate::services::nextendo;
use std::net::Ipv4Addr;

const ADDRINFO_MAGIC: u32 = 0xBEEF_CAFE;
const NETDB_HOST_NOT_FOUND: u32 = 1;
const GAI_NODATA: u32 = 7;
const GAI_SERVICE: u32 = 9;
const GAI_FAMILY: u32 = 5;
const GAI_OVERFLOW: u32 = 14;

pub struct Resolver;

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]).into_owned()
}

fn lookup(cmd: u32, host: &str) -> Vec<Ipv4Addr> {
    let host = nextendo::nsd_resolve(host);
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return vec![ip];
    }
    match nextendo::redirect(&host) {
        Some(ip) => {
            eprintln!("[sfdnsres] cmd {cmd}: {host} -> {ip} (nextendo)");
            vec![ip]
        }
        None => {
            eprintln!("[sfdnsres] cmd {cmd}: {host} blocked");
            Vec::new()
        }
    }
}

fn addrinfos(host: &str, ips: &[Ipv4Addr], port: u16) -> Vec<u8> {
    let mut out = Vec::new();
    for ip in ips {
        for v in [ADDRINFO_MAGIC, 0, libc::AF_INET as u32, 0, 0, 16] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.extend_from_slice(&[16, libc::AF_INET as u8]);
        out.extend_from_slice(&port.to_le_bytes());
        out.extend(ip.octets().iter().rev());
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(host.as_bytes());
        out.push(0);
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

fn hostent(host: &str, ips: &[Ipv4Addr]) -> Vec<u8> {
    let mut out = host.as_bytes().to_vec();
    out.push(0);
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&(libc::AF_INET as u16).to_be_bytes());
    out.extend_from_slice(&4u16.to_be_bytes());
    out.extend_from_slice(&(ips.len() as u32).to_be_bytes());
    for ip in ips {
        out.extend(ip.octets().iter().rev());
    }
    out
}

fn reply(rsp: &mut Response, with_options: bool, size: usize, errno: u32, netdb: u32) {
    if with_options {
        for v in [size as u32, errno, netdb, 0] {
            rsp.push_u32(v);
        }
    } else {
        for v in [netdb, errno, size as u32] {
            rsp.push_u32(v);
        }
    }
}

fn name_info(req: &Request, rsp: &mut Response, with_options: bool) {
    let addr = bytes(in_buf(req, 0));
    let mut gai = GAI_FAMILY;
    if addr.len() >= 8 && addr[1] == libc::AF_INET as u8 {
        let host = Ipv4Addr::new(addr[4], addr[5], addr[6], addr[7]).to_string();
        let service = u16::from_be_bytes([addr[2], addr[3]]).to_string();
        let (h, s) = (out_buf(req, 0), out_buf(req, 1));
        gai = if h.is_some_and(|b| host.len() as u64 >= b.size) || s.is_some_and(|b| service.len() as u64 >= b.size) {
            GAI_OVERFLOW
        } else {
            for (b, text) in [(h, host), (s, service)] {
                if let Some(b) = b {
                    write_buffer(&b, format!("{text}\0").as_bytes());
                }
            }
            0
        };
    }
    rsp.push_u32(gai);
    if with_options {
        rsp.push_u32(if gai == 0 { 0 } else { u32::MAX });
    }
    rsp.push_u32(if gai == 0 { 0 } else { 22 });
}

impl Service for Resolver {
    fn name(&self) -> String {
        "sfdnsres".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            2 | 10 => {
                let host = cstr(bytes(in_buf(req, 0)));
                let ips = lookup(req.cmd, &host);
                let out = out_buf(req, 0);
                if ips.is_empty() {
                    reply(rsp, req.cmd == 10, 0, GAI_NODATA, NETDB_HOST_NOT_FOUND);
                } else {
                    let n = out.map_or(0, |b| write_buffer(&b, &hostent(&nextendo::nsd_resolve(&host), &ips)));
                    reply(rsp, req.cmd == 10, n, 0, 0);
                }
            }
            6 | 12 => {
                let host = cstr(bytes(in_buf(req, 0)));
                let service = cstr(bytes(in_buf(req, 1)));
                let Ok(port) = (if service.is_empty() { Ok(0) } else { service.parse::<u16>() }) else {
                    reply(rsp, req.cmd == 12, 0, GAI_SERVICE, 0);
                    return 0;
                };
                let ips = lookup(req.cmd, &host);
                if ips.is_empty() {
                    reply(rsp, req.cmd == 12, 0, GAI_NODATA, NETDB_HOST_NOT_FOUND);
                } else {
                    let data = addrinfos(&nextendo::nsd_resolve(&host), &ips, port);
                    let n = out_buf(req, 0).map_or(0, |b| write_buffer(&b, &data));
                    reply(rsp, req.cmd == 12, n, 0, 0);
                }
            }
            4 | 5 => {
                if let Some(b) = out_buf(req, 0) {
                    write_buffer(&b, if req.u32(0) == 0 { b"Success\0" } else { b"Unknown host\0" });
                }
            }
            7 | 13 => name_info(req, rsp, req.cmd == 13),
            8 => rsp.push_u32(0),
            9 | 65000 => {}
            14 => {
                rsp.push_u32(0);
                rsp.push_u32(0);
            }
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_nextendo_names_resolve() {
        assert!(lookup(6, "example.com").is_empty());
        assert!(lookup(6, "www.nintendo-europe.com").is_empty());
        assert_eq!(lookup(6, "g2b309e01-%.s.n.srv.nintendo.net"), vec![Ipv4Addr::new(164, 132, 111, 120)]);
    }

    #[test]
    fn hostent_layout() {
        let d = hostent("a", &[Ipv4Addr::new(164, 132, 111, 120)]);
        assert_eq!(&d[..2], b"a\0");
        assert_eq!(&d[2..14], &[0, 0, 0, 0, 0, 2, 0, 4, 0, 0, 0, 1]);
        assert_eq!(&d[14..], &[120, 111, 132, 164]);
    }

    #[test]
    fn addrinfo_layout() {
        let d = addrinfos("a", &[Ipv4Addr::new(1, 2, 3, 4)], 443);
        assert_eq!(&d[..4], &[0xBE, 0xEF, 0xCA, 0xFE]);
        assert_eq!(&d[20..24], &16u32.to_be_bytes());
        assert_eq!(&d[24..32], &[16, 2, 0xBB, 0x01, 4, 3, 2, 1]);
        assert_eq!(&d[40..42], b"a\0");
        assert_eq!(&d[42..], &[0, 0, 0, 0]);
    }
}
