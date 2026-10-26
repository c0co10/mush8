// Mush8 - webpad.rs
// Constantin Clerc (c22dev) - 0.1
use crate::remote::{self, Pad};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

const PORT: u16 = 8088;
const PAGE: &str = include_str!("webpad.html");
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const STATUS: Duration = Duration::from_millis(500);
const SILENCE: Duration = Duration::from_secs(3);
const LIMIT: usize = 1 << 16;

pub fn enabled() -> bool {
    std::env::var("NX_BROWSER_PADS").is_ok_and(|v| v == "1")
}

pub fn host() {
    match TcpListener::bind(("0.0.0.0", PORT)) {
        Ok(listener) => {
            eprintln!("[webpad] browser controllers on TCP {PORT}");
            serve(listener);
        }
        Err(e) => eprintln!("[webpad] can't listen for browser controllers on TCP {PORT}: {e}"),
    }
}

fn serve(listener: TcpListener) {
    let _ = std::thread::Builder::new().name("webpad".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = std::thread::Builder::new().name("webpad-client".into()).spawn(move || client(stream));
        }
    });
}

struct Request {
    path: String,
    key: Option<String>,
    rest: Vec<u8>,
}

fn client(mut s: TcpStream) {
    let Ok(peer) = s.peer_addr() else { return };
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let Some(r) = request(&mut s) else { return };
    match (r.path.as_str(), r.key) {
        ("/ws", Some(key)) => socket(s, peer, &key, r.rest),
        ("/" | "/index.html", _) => respond(&mut s, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()),
        _ => respond(&mut s, "404 Not Found", "text/plain", b"not found"),
    }
}

fn request(s: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > LIMIT {
            return None;
        }
        let n = s.read(&mut chunk).ok().filter(|&n| n > 0)?;
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let target = lines.next()?.split(' ').nth(1)?;
    let path = target.split('?').next()?;
    let key = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("sec-websocket-key"))
        .map(|(_, v)| v.trim().to_string());
    Some(Request { path: path.to_string(), key, rest: buf[end..].to_vec() })
}

fn respond(s: &mut TcpStream, status: &str, kind: &str, body: &[u8]) {
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes()).and_then(|_| s.write_all(body));
}

fn socket(mut s: TcpStream, peer: SocketAddr, key: &str, mut buf: Vec<u8>) {
    let reply = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", accept(key));
    if s.write_all(reply.as_bytes()).is_err() || s.set_read_timeout(Some(Duration::from_millis(100))).is_err() {
        return;
    }
    let mut chunk = [0u8; 2048];
    let mut heard = Instant::now();
    let mut told: Option<Instant> = None;
    'open: loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            Err(_) => break,
        }
        while let Some((op, payload, used)) = frame(&buf) {
            buf.drain(..used);
            match op {
                0x1 => {
                    if let Some(pad) = parse(&payload) {
                        if !remote::feed(peer, pad) {
                            break 'open;
                        }
                        heard = Instant::now();
                    }
                }
                0x8 => break 'open,
                0x9 if payload.len() < 126 && s.write_all(&encode(0xA, &payload)).is_err() => break 'open,
                _ => {}
            }
        }
        if buf.len() > LIMIT || heard.elapsed() > SILENCE {
            break;
        }
        if told.is_none_or(|t| t.elapsed() >= STATUS) {
            told = Some(Instant::now());
            let status = remote::player(peer).map(|p| encode(0x1, format!("player {p}").as_bytes()));
            if status.is_some_and(|f| s.write_all(&f).is_err()) {
                break;
            }
        }
    }
    remote::forget(peer);
}

fn frame(b: &[u8]) -> Option<(u8, Vec<u8>, usize)> {
    let (&b0, &b1) = (b.first()?, b.get(1)?);
    let (len, mut at) = match b1 & 0x7F {
        126 => (u16::from_be_bytes(b.get(2..4)?.try_into().ok()?) as usize, 4),
        127 => (usize::try_from(u64::from_be_bytes(b.get(2..10)?.try_into().ok()?)).ok()?, 10),
        n => (n as usize, 2),
    };
    let mut mask = [0u8; 4];
    if b1 & 0x80 != 0 {
        mask = b.get(at..at + 4)?.try_into().ok()?;
        at += 4;
    }
    let data = b.get(at..at.checked_add(len)?)?;
    Some((b0 & 0x0F, data.iter().enumerate().map(|(i, c)| c ^ mask[i % 4]).collect(), at + len))
}

fn encode(op: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![0x80 | op];
    match payload.len() {
        n if n < 126 => f.push(n as u8),
        n => {
            f.push(126);
            f.extend_from_slice(&(n as u16).to_be_bytes());
        }
    }
    f.extend_from_slice(payload);
    f
}

fn parse(payload: &[u8]) -> Option<Pad> {
    let mut f = std::str::from_utf8(payload).ok()?.split(',');
    let buttons = f.next()?.trim().parse::<u64>().ok()? & 0xFFFF;
    let mut axis = || f.next()?.trim().parse::<f32>().ok().filter(|v| v.is_finite()).map(|v| v.clamp(-1.0, 1.0));
    let (lx, ly) = (axis()?, axis()?);
    Some(Pad { buttons, sticks: [lx, ly, 0.0, 0.0], motion: None })
}

fn accept(key: &str) -> String {
    base64(&sha1(format!("{key}{GUID}").as_bytes()))
}

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut m = data.to_vec();
    m.push(0x80);
    while m.len() % 64 != 56 {
        m.push(0);
    }
    m.extend_from_slice(&(data.len() as u64 * 8).to_be_bytes());
    for block in m.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes(word.try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wi);
            (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t);
        }
        for (x, v) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(v);
        }
    }
    let mut out = [0u8; 20];
    for (o, v) in out.chunks_mut(4).zip(h) {
        o.copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { T[(n >> (18 - 6 * i)) as usize & 63] as char } else { '=' });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masked(op: u8, payload: &[u8]) -> Vec<u8> {
        let mask = [0x12, 0x34, 0x56, 0x78];
        let mut f = vec![0x80 | op, 0x80 | payload.len() as u8];
        f.extend_from_slice(&mask);
        f.extend(payload.iter().enumerate().map(|(i, c)| c ^ mask[i % 4]));
        f
    }

    #[test]
    fn handshake() {
        assert_eq!(base64(&sha1(b"abc")), "qZk+NkcGgWq6PiVxeFDCbJzQ2J0=");
        assert_eq!(base64(&sha1(b"")), "2jmj7l5rSw0yVb/vlWAYkK/YBwk=");
        assert_eq!(accept("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }

    #[test]
    fn frames() {
        let mut b = masked(0x1, b"129,0.500,-1.000");
        b.extend(masked(0x8, b""));
        let (op, payload, used) = frame(&b).unwrap();
        assert_eq!((op, payload.as_slice()), (0x1, b"129,0.500,-1.000".as_slice()));
        assert_eq!(frame(&b[used..]).unwrap().0, 0x8);
        assert!(frame(&b[..5]).is_none());
        assert_eq!(frame(&encode(0x1, b"player 2")).unwrap().1, b"player 2");
        let long = vec![7u8; 300];
        assert_eq!(frame(&encode(0x1, &long)).unwrap().1, long);
    }

    #[test]
    fn pads() {
        let p = parse(b"129,0.500,-1.000").unwrap();
        assert_eq!((p.buttons, p.sticks), (129, [0.5, -1.0, 0.0, 0.0]));
        assert!(p.motion.is_none());
        assert_eq!(parse(b"70000,9,-9").unwrap().sticks, [1.0, -1.0, 0.0, 0.0]);
        assert_eq!(parse(b"70000,0,0").unwrap().buttons, 70000 & 0xFFFF);
        assert!(parse(b"1,NaN,0").is_none());
        assert!(parse(b"nope").is_none());
    }

    #[test]
    fn serves_page_and_feeds_pads() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let at = listener.local_addr().unwrap();
        serve(listener);

        let mut page = TcpStream::connect(at).unwrap();
        page.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut html = String::new();
        page.read_to_string(&mut html).unwrap();
        assert!(html.starts_with("HTTP/1.1 200 OK") && html.contains("<title>Mush8 Pad</title>"));

        let mut ws = TcpStream::connect(at).unwrap();
        let me = ws.local_addr().unwrap();
        ws.write_all(b"GET /ws?id=0123 HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").unwrap();
        let mut head = [0u8; 256];
        let n = ws.read(&mut head).unwrap();
        let head = String::from_utf8_lossy(&head[..n]);
        assert!(head.starts_with("HTTP/1.1 101") && head.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));

        ws.write_all(&masked(0x1, b"129,0.500,-1.000")).unwrap();
        let fed = (0..100).find_map(|_| {
            std::thread::sleep(Duration::from_millis(10));
            remote::connected().into_iter().find(|(d, _)| *d == remote::device_id(me))
        });
        let pad = fed.expect("pad never reached remote").1;
        assert_eq!((pad.buttons, pad.sticks), (129, [0.5, -1.0, 0.0, 0.0]));
        assert!(remote::is_web(remote::device_id(me)));

        remote::set_player(remote::device_id(me), 2);
        ws.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut got = Vec::new();
        let mut chunk = [0u8; 64];
        while !got.windows(8).any(|w| w == b"player 2") {
            let n = ws.read(&mut chunk).unwrap();
            assert!(n > 0);
            got.extend_from_slice(&chunk[..n]);
        }

        ws.write_all(&masked(0x8, b"")).unwrap();
        let gone = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            !remote::connected().iter().any(|(d, _)| *d == remote::device_id(me))
        });
        assert!(gone, "pad still listed after the socket closed");
    }
}
