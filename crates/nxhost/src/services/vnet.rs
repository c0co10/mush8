// Mush8 - vnet.rs
// Constantin Clerc (c22dev) - 0.1
use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

pub trait Transport: Send + Sync + 'static {
    fn send(&self, packet: &[u8]);
}

pub const MASK: [u8; 4] = [255, 255, 0, 0];
pub const GATEWAY: [u8; 4] = [10, 13, 37, 1];
const MTU: usize = 1500;
const QUEUE: usize = 512;

const EAGAIN: u32 = 11;
const EBADF: u32 = 9;
const EINVAL: u32 = 22;
const EDESTADDRREQ: u32 = 89;
const EMSGSIZE: u32 = 90;
const EADDRNOTAVAIL: u32 = 99;
const ENOTCONN: u32 = 107;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    Lan,
    LanPlay,
    Wireless,
    Nextendo,
}

impl Mode {
    pub fn is_virtual(self) -> bool {
        matches!(self, Mode::LanPlay | Mode::Wireless)
    }
}

pub fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("NX_NET").ok().as_deref() {
        Some("lan") => Mode::Lan,
        Some("lanplay") => Mode::LanPlay,
        Some("wireless") => Mode::Wireless,
        Some("nextendo") => Mode::Nextendo,
        Some("off") => Mode::Off,
        None if std::env::var_os("NX_LAN").is_some() => Mode::Lan,
        None => Mode::Off,
        Some(other) => {
            eprintln!("[vnet] unknown NX_NET={other} (off, lan, lanplay, wireless, nextendo), network off");
            Mode::Off
        }
    })
}

fn transport(mode: Mode) -> Option<Box<dyn Transport>> {
    match mode {
        Mode::LanPlay => match std::env::var("NX_LANPLAY_SERVER") {
            Ok(server) if !server.trim().is_empty() => crate::services::lanplay::start(server.trim(), address()),
            _ => {
                eprintln!("[vnet] lanplay needs NX_LANPLAY_SERVER=host[:port]");
                None
            }
        },
        Mode::Wireless => crate::services::wireless::start(address()),
        _ => None,
    }
}

fn link() -> Option<&'static dyn Transport> {
    static LINK: OnceLock<Option<Box<dyn Transport>>> = OnceLock::new();
    LINK.get_or_init(|| {
        let t = transport(mode())?;
        eprintln!("[vnet] this device is {}", Ipv4Addr::from(address()));
        Some(t)
    })
    .as_deref()
}

pub fn address() -> [u8; 4] {
    static ADDR: OnceLock<[u8; 4]> = OnceLock::new();
    *ADDR.get_or_init(|| loop {
        let mut r = [0u8; 2];
        unsafe { libc::arc4random_buf(r.as_mut_ptr().cast(), 2) };
        let a = [10, 13, r[0], r[1]];
        if (1..=254).contains(&r[0]) && (1..=254).contains(&r[1]) && a != GATEWAY {
            break a;
        }
    })
}

pub fn network() -> Option<([u8; 4], [u8; 4], [u8; 4])> {
    link().map(|_| (address(), MASK, GATEWAY))
}

fn is_broadcast(ip: [u8; 4]) -> bool {
    let own = address();
    ip == [255; 4] || ip == std::array::from_fn(|i| own[i] | !MASK[i])
}

fn is_local(ip: [u8; 4]) -> bool {
    ip == address() || ip[0] == 127
}

static EPOCH: Mutex<u64> = Mutex::new(0);
static CHANGED: Condvar = Condvar::new();

fn changed() {
    *EPOCH.lock().unwrap() += 1;
    CHANGED.notify_all();
}

pub fn epoch() -> u64 {
    *EPOCH.lock().unwrap()
}

pub fn wait_change(seen: u64, limit: Option<Duration>) {
    let g = EPOCH.lock().unwrap();
    match limit {
        Some(d) => drop(CHANGED.wait_timeout_while(g, d, |e| *e == seen)),
        None => drop(CHANGED.wait_while(g, |e| *e == seen)),
    }
}

fn ports() -> &'static Mutex<HashMap<u16, Vec<Weak<Socket>>>> {
    static PORTS: OnceLock<Mutex<HashMap<u16, Vec<Weak<Socket>>>>> = OnceLock::new();
    PORTS.get_or_init(Default::default)
}

#[derive(Default)]
struct State {
    local: Option<SocketAddrV4>,
    peer: Option<SocketAddrV4>,
    nonblock: bool,
    timeout: Option<Duration>,
    queue: VecDeque<(SocketAddrV4, Vec<u8>)>,
    shut: bool,
    closed: bool,
}

pub struct Socket {
    st: Mutex<State>,
    cv: Condvar,
    refs: AtomicUsize,
}

pub const POLLIN: i16 = 1;
pub const POLLOUT: i16 = 4;
pub const POLLNVAL: i16 = 0x20;

impl Socket {
    pub fn new() -> Arc<Self> {
        Arc::new(Socket { st: Mutex::default(), cv: Condvar::new(), refs: AtomicUsize::new(1) })
    }

    pub fn bind(self: &Arc<Self>, at: SocketAddrV4) -> Result<(), u32> {
        let ip = at.ip().octets();
        if ip != [0; 4] && !is_local(ip) {
            return Err(EADDRNOTAVAIL);
        }
        let mut all = ports().lock().unwrap();
        let mut s = self.st.lock().unwrap();
        if s.local.is_some() || s.closed {
            return Err(EINVAL);
        }
        let port = match at.port() {
            0 => ephemeral(&all),
            p => p,
        };
        let list = all.entry(port).or_default();
        list.retain(|w| w.strong_count() > 0);
        list.push(Arc::downgrade(self));
        s.local = Some(SocketAddrV4::new(*at.ip(), port));
        Ok(())
    }

    fn bound(self: &Arc<Self>) -> SocketAddrV4 {
        let _ = self.bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
        self.st.lock().unwrap().local.unwrap_or(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
    }

    pub fn connect(self: &Arc<Self>, peer: Option<SocketAddrV4>) -> Result<(), u32> {
        self.bound();
        let mut s = self.st.lock().unwrap();
        s.peer = peer.map(|p| if p.ip().is_unspecified() { SocketAddrV4::new(Ipv4Addr::LOCALHOST, p.port()) } else { p });
        if let Some(p) = s.peer {
            s.queue.retain(|(from, _)| *from == p);
        }
        Ok(())
    }

    pub fn send_to(self: &Arc<Self>, data: &[u8], to: Option<SocketAddrV4>) -> Result<usize, u32> {
        let dst = to.or(self.st.lock().unwrap().peer).ok_or(EDESTADDRREQ)?;
        if data.len() > 65507 {
            return Err(EMSGSIZE);
        }
        let port = self.bound().port();
        let ip = dst.ip().octets();
        let src = SocketAddrV4::new(if ip[0] == 127 { Ipv4Addr::LOCALHOST } else { address().into() }, port);
        if is_local(ip) || is_broadcast(ip) {
            deliver(src, dst, data);
        }
        if !is_local(ip) {
            if let Some(t) = link() {
                for p in packets(src, dst, data) {
                    t.send(&p);
                }
            }
        }
        Ok(data.len())
    }

    pub fn recv_from(&self, buf: &mut [u8], peek: bool, dontwait: bool) -> Result<(usize, SocketAddrV4), u32> {
        let mut deadline = None;
        loop {
            let mut s = self.st.lock().unwrap();
            if s.closed {
                return Err(EBADF);
            }
            if let Some((from, d)) = if peek { s.queue.front().cloned() } else { s.queue.pop_front() } {
                let n = d.len().min(buf.len());
                buf[..n].copy_from_slice(&d[..n]);
                return Ok((n, from));
            }
            if s.shut {
                return Ok((0, SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)));
            }
            if s.nonblock || dontwait {
                return Err(EAGAIN);
            }
            let limit = *deadline.get_or_insert_with(|| s.timeout.map(|t| Instant::now() + t));
            if limit.is_some_and(|d| Instant::now() >= d) {
                return Err(EAGAIN);
            }
            drop(s);
            crate::kernel::blocking(|| {
                let s = self.st.lock().unwrap();
                let idle = |s: &mut State| s.queue.is_empty() && !s.shut && !s.closed;
                match limit {
                    Some(d) => drop(self.cv.wait_timeout_while(s, d.saturating_duration_since(Instant::now()), idle)),
                    None => drop(self.cv.wait_while(s, idle)),
                }
            });
        }
    }

    pub fn local(&self) -> SocketAddrV4 {
        let s = self.st.lock().unwrap();
        let local = s.local.unwrap_or(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
        match s.peer {
            Some(p) if local.ip().is_unspecified() && s.local.is_some() => {
                SocketAddrV4::new(if p.ip().is_loopback() { Ipv4Addr::LOCALHOST } else { address().into() }, local.port())
            }
            _ => local,
        }
    }

    pub fn peer(&self) -> Result<SocketAddrV4, u32> {
        self.st.lock().unwrap().peer.ok_or(ENOTCONN)
    }

    pub fn nonblocking(&self) -> bool {
        self.st.lock().unwrap().nonblock
    }

    pub fn set_nonblocking(&self, on: bool) {
        self.st.lock().unwrap().nonblock = on;
    }

    pub fn set_timeout(&self, t: Option<Duration>) {
        self.st.lock().unwrap().timeout = t;
    }

    pub fn pending(&self) -> usize {
        self.st.lock().unwrap().queue.front().map_or(0, |(_, d)| d.len())
    }

    pub fn poll(&self, events: i16) -> i16 {
        let s = self.st.lock().unwrap();
        if s.closed {
            return POLLNVAL;
        }
        let readable = !s.queue.is_empty() || s.shut;
        events & ((if readable { POLLIN } else { 0 }) | POLLOUT)
    }

    pub fn shutdown(&self) {
        self.st.lock().unwrap().shut = true;
        self.cv.notify_all();
        changed();
    }

    pub fn dup(&self) {
        self.refs.fetch_add(1, Relaxed);
    }

    pub fn close(&self) {
        if self.refs.fetch_sub(1, Relaxed) == 1 {
            let mut s = self.st.lock().unwrap();
            s.closed = true;
            s.queue.clear();
            drop(s);
            self.cv.notify_all();
            changed();
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        if let Some(port) = self.st.get_mut().map(|s| s.local.map(|l| l.port())).ok().flatten() {
            if let Ok(mut all) = ports().lock() {
                if let Some(list) = all.get_mut(&port) {
                    list.retain(|w| w.strong_count() > 0);
                    if list.is_empty() {
                        all.remove(&port);
                    }
                }
            }
        }
    }
}

fn ephemeral(used: &HashMap<u16, Vec<Weak<Socket>>>) -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let base = 50000u16;
    let mut r = [0u8; 2];
    unsafe { libc::arc4random_buf(r.as_mut_ptr().cast(), 2) };
    let _ = NEXT.compare_exchange(0, u16::from_ne_bytes(r) % 10000, Relaxed, Relaxed);
    loop {
        let p = base + NEXT.fetch_add(1, Relaxed) % 10000;
        if used.get(&p).is_none_or(|l| l.iter().all(|w| w.strong_count() == 0)) {
            return p;
        }
    }
}

fn deliver(src: SocketAddrV4, dst: SocketAddrV4, data: &[u8]) {
    let socks: Vec<Arc<Socket>> = ports().lock().unwrap().get(&dst.port()).map(|l| l.iter().filter_map(Weak::upgrade).collect()).unwrap_or_default();
    let bcast = is_broadcast(dst.ip().octets());
    for s in socks {
        let mut st = s.st.lock().unwrap();
        let bound = st.local.map_or(Ipv4Addr::UNSPECIFIED, |l| *l.ip());
        if st.closed || st.queue.len() >= QUEUE || st.peer.is_some_and(|p| p != src) || (!bound.is_unspecified() && (bcast || bound != *dst.ip())) {
            continue;
        }
        st.queue.push_back((src, data.to_vec()));
        drop(st);
        s.cv.notify_all();
    }
    changed();
}

fn checksum(parts: &[&[u8]]) -> u16 {
    let mut sum = 0u32;
    for p in parts {
        for c in p.chunks(2) {
            sum += u32::from(u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]));
        }
    }
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn packets(src: SocketAddrV4, dst: SocketAddrV4, data: &[u8]) -> Vec<Vec<u8>> {
    static ID: AtomicU16 = AtomicU16::new(1);
    let (s, d) = (src.ip().octets(), dst.ip().octets());
    let len = (8 + data.len()) as u16;
    let mut udp = Vec::with_capacity(len as usize);
    udp.extend_from_slice(&src.port().to_be_bytes());
    udp.extend_from_slice(&dst.port().to_be_bytes());
    udp.extend_from_slice(&len.to_be_bytes());
    udp.extend_from_slice(&[0, 0]);
    udp.extend_from_slice(data);
    let sum = checksum(&[&s, &d, &[0, 17], &len.to_be_bytes(), &udp]);
    udp[6..8].copy_from_slice(&(if sum == 0 { 0xFFFF } else { sum }).to_be_bytes());
    let id = ID.fetch_add(1, Relaxed);
    let chunk = (MTU - 20) & !7;
    let n = udp.len().div_ceil(chunk);
    udp.chunks(chunk)
        .enumerate()
        .map(|(i, part)| {
            let frag = ((i * chunk / 8) as u16) | if i + 1 < n { 0x2000 } else { 0 };
            let mut p = Vec::with_capacity(20 + part.len());
            p.extend_from_slice(&[0x45, 0]);
            p.extend_from_slice(&((20 + part.len()) as u16).to_be_bytes());
            p.extend_from_slice(&id.to_be_bytes());
            p.extend_from_slice(&frag.to_be_bytes());
            p.extend_from_slice(&[64, 17, 0, 0]);
            p.extend_from_slice(&s);
            p.extend_from_slice(&d);
            let sum = checksum(&[&p]);
            p[10..12].copy_from_slice(&sum.to_be_bytes());
            p.extend_from_slice(part);
            p
        })
        .collect()
}

struct Partial {
    parts: Vec<(usize, Vec<u8>)>,
    total: Option<usize>,
    since: Instant,
}

fn reassemble(src: [u8; 4], id: u16, offset: usize, more: bool, data: &[u8]) -> Option<Vec<u8>> {
    static PARTIAL: OnceLock<Mutex<HashMap<([u8; 4], u16), Partial>>> = OnceLock::new();
    let mut all = PARTIAL.get_or_init(Default::default).lock().unwrap();
    all.retain(|_, p| p.since.elapsed() < Duration::from_secs(5));
    let p = all.entry((src, id)).or_insert_with(|| Partial { parts: Vec::new(), total: None, since: Instant::now() });
    if !more {
        p.total = Some(offset + data.len());
    }
    if !p.parts.iter().any(|(o, _)| *o == offset) {
        p.parts.push((offset, data.to_vec()));
    }
    let total = p.total?;
    if p.parts.iter().map(|(_, d)| d.len()).sum::<usize>() < total {
        return None;
    }
    let p = all.remove(&(src, id))?;
    let mut out = vec![0; total];
    for (o, d) in p.parts {
        out.get_mut(o..o + d.len())?.copy_from_slice(&d);
    }
    Some(out)
}

pub fn receive(packet: &[u8]) {
    if packet.len() < 20 || packet[0] >> 4 != 4 || packet[9] != 17 {
        return;
    }
    let hl = usize::from(packet[0] & 0xF) * 4;
    let total = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
    if hl < 20 || total < hl || total > packet.len() || checksum(&[&packet[..hl]]) != 0 {
        return;
    }
    let src: [u8; 4] = packet[12..16].try_into().unwrap();
    let dst: [u8; 4] = packet[16..20].try_into().unwrap();
    if src == address() || (dst != address() && !is_broadcast(dst)) {
        return;
    }
    let frag = u16::from_be_bytes([packet[6], packet[7]]);
    let body = &packet[hl..total];
    let whole;
    let udp = if frag & 0x3FFF != 0 {
        let id = u16::from_be_bytes([packet[4], packet[5]]);
        match reassemble(src, id, usize::from(frag & 0x1FFF) * 8, frag & 0x2000 != 0, body) {
            Some(w) => {
                whole = w;
                &whole[..]
            }
            None => return,
        }
    } else {
        body
    };
    if udp.len() < 8 {
        return;
    }
    let len = usize::from(u16::from_be_bytes([udp[4], udp[5]]));
    if len < 8 || len > udp.len() {
        return;
    }
    let port = |o: usize| u16::from_be_bytes([udp[o], udp[o + 1]]);
    deliver(SocketAddrV4::new(src.into(), port(0)), SocketAddrV4::new(dst.into(), port(2)), &udp[8..len]);
}
