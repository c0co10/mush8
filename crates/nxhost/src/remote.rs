// Mush8 - remote.rs
// Constantin Clerc (c22dev) - 0.1
use crate::input::Motion;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TYPE: &str = "_mush8-pad._udp";
const PORT: u16 = 21421;
const PAD: [u8; 4] = *b"M8P1";
const ACK: [u8; 4] = *b"M8A1";
const LEN: usize = 4 + 8 + 16 + 1 + 24;
const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Default)]
pub struct Pad {
    pub buttons: u64,
    pub sticks: [f32; 4],
    pub motion: Option<Motion>,
}

impl Pad {
    #[cfg_attr(not(target_os = "ios"), allow(dead_code))]
    fn encode(&self) -> [u8; LEN] {
        let mut b = [0u8; LEN];
        b[..4].copy_from_slice(&PAD);
        b[4..12].copy_from_slice(&self.buttons.to_le_bytes());
        let m = self.motion.unwrap_or(Motion { accel: [0.0; 3], gyro: [0.0; 3] });
        for (i, f) in self.sticks.iter().chain(&m.accel).chain(&m.gyro).enumerate() {
            let o = if i < 4 { 12 + 4 * i } else { 29 + 4 * (i - 4) };
            b[o..o + 4].copy_from_slice(&f.to_le_bytes());
        }
        b[28] = self.motion.is_some() as u8;
        b
    }

    fn decode(b: &[u8]) -> Option<Pad> {
        if b.len() < LEN || b[..4] != PAD {
            return None;
        }
        let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap()).clamp(-64.0, 64.0);
        let v = |o: usize| [f(o), f(o + 4), f(o + 8)];
        Some(Pad {
            buttons: u64::from_le_bytes(b[4..12].try_into().unwrap()),
            sticks: [f(12), f(16), f(20), f(24)].map(|x| x.clamp(-1.0, 1.0)),
            motion: (b[28] != 0).then(|| Motion { accel: v(29), gyro: v(41) }),
        })
    }
}

struct Phone {
    addr: SocketAddr,
    pad: Pad,
    seen: Instant,
    player: Option<u8>,
    acked: Option<(Instant, u8)>,
    web: bool,
    id: u64,
}

static PHONES: Mutex<Vec<Phone>> = Mutex::new(Vec::new());
static RETIRED: Mutex<Vec<SocketAddr>> = Mutex::new(Vec::new());

pub fn enabled() -> bool {
    let default = cfg!(target_os = "macos");
    std::env::var("NX_PHONE_PADS").map_or(default, |v| v != "0")
}

pub fn host() {
    let sock = match UdpSocket::bind(("0.0.0.0", PORT)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[phone] can't listen for iPhone controllers on UDP {PORT}: {e}");
            return;
        }
    };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(250)));
    let _ = std::thread::Builder::new().name("phone-pads".into()).spawn(move || {
        let mut buf = [0u8; 256];
        loop {
            let got = sock.recv_from(&mut buf);
            let now = Instant::now();
            let mut phones = PHONES.lock().unwrap();
            if let Ok((n, addr)) = got {
                if let Some(pad) = Pad::decode(&buf[..n]) {
                    match phones.iter_mut().find(|p| p.addr == addr) {
                        Some(p) => (p.pad, p.seen) = (pad, now),
                        None => {
                            eprintln!("[phone] iPhone controller connected from {addr}");
                            phones.push(Phone { addr, pad, seen: now, player: None, acked: None, web: false, id: device_id(addr) });
                        }
                    }
                }
            }
            phones.retain(|p| {
                let live = now.duration_since(p.seen) < TIMEOUT;
                if !live {
                    eprintln!("[phone] {} controller at {} disconnected", if p.web { "browser" } else { "iPhone" }, p.addr);
                }
                live
            });
            for p in phones.iter_mut().filter(|p| !p.web) {
                let Some(player) = p.player else { continue };
                if p.acked.is_none_or(|(at, was)| was != player || now.duration_since(at) > Duration::from_millis(500)) {
                    let mut ack = [0u8; 5];
                    ack[..4].copy_from_slice(&ACK);
                    ack[4] = player;
                    let _ = sock.send_to(&ack, p.addr);
                    p.acked = Some((now, player));
                }
            }
        }
    });
    let _ = std::thread::Builder::new().name("phone-bonjour".into()).spawn(|| loop {
        advertise();
        std::thread::sleep(Duration::from_secs(5));
    });
}

fn advertise() {
    use crate::dnssd::*;
    let typ = std::ffi::CString::new(TYPE).unwrap();
    unsafe {
        let mut sd: DNSServiceRef = std::ptr::null_mut();
        let r = DNSServiceRegister(&mut sd, 0, 0, std::ptr::null(), typ.as_ptr(), std::ptr::null(), std::ptr::null(), PORT.to_be(), 0, std::ptr::null(), None, std::ptr::null_mut());
        if r != 0 {
            eprintln!("[phone] bonjour did not start ({r}){}", denied(r));
            return;
        }
        eprintln!("[phone] iPhones can join as controllers ({TYPE})");
        while wait(sd, -1) && DNSServiceProcessResult(sd) == 0 {}
        DNSServiceRefDeallocate(sd);
    }
}

pub fn connected() -> Vec<(u64, Pad)> {
    PHONES.lock().unwrap().iter().map(|p| (p.id, p.pad)).collect()
}

pub fn set_player(device: u64, player: u8) {
    for p in PHONES.lock().unwrap().iter_mut().filter(|p| p.id == device) {
        p.player = Some(player);
    }
}

pub fn device_id(addr: SocketAddr) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    addr.ip().hash(&mut h);
    h.finish()
}

pub fn feed(addr: SocketAddr, pad: Pad) -> bool {
    let id = device_id(addr);
    let mut phones = PHONES.lock().unwrap();
    let mut retired = RETIRED.lock().unwrap();
    if retired.contains(&addr) {
        return false;
    }
    if let Some(p) = phones.iter_mut().find(|p| p.addr == addr) {
        (p.pad, p.seen) = (pad, Instant::now());
    } else if let Some(p) = phones.iter_mut().find(|p| p.web && p.id == id) {
        eprintln!("[phone] browser controller moved from {} to {addr}", p.addr);
        retired.push(p.addr);
        (p.addr, p.pad, p.seen) = (addr, pad, Instant::now());
    } else {
        eprintln!("[phone] browser controller connected from {addr}");
        phones.push(Phone { addr, pad, seen: Instant::now(), player: None, acked: None, web: true, id });
    }
    true
}

pub fn player(addr: SocketAddr) -> Option<u8> {
    PHONES.lock().unwrap().iter().find(|p| p.addr == addr).and_then(|p| p.player)
}

pub fn forget(addr: SocketAddr) {
    let mut phones = PHONES.lock().unwrap();
    if phones.iter().any(|p| p.addr == addr) {
        eprintln!("[phone] browser controller at {addr} disconnected");
        phones.retain(|p| p.addr != addr);
    }
    RETIRED.lock().unwrap().retain(|&a| a != addr);
}

pub fn is_web(device: u64) -> bool {
    PHONES.lock().unwrap().iter().any(|p| p.id == device && p.web)
}

#[cfg(target_os = "ios")]
pub use phone::run as run_controller;

#[cfg(target_os = "ios")]
mod phone {
    use super::*;
    use crate::dnssd::*;
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::sync::atomic::Ordering::Relaxed;

    struct Host {
        name: String,
        addr: SocketAddr,
    }

    pub fn run(status: fn(&str)) {
        let sock = UdpSocket::bind(("0.0.0.0", 0)).expect("udp socket");
        let _ = sock.set_nonblocking(true);
        let mut motion = None;
        eprintln!("[phone] controller mode: looking for {TYPE}");
        status(&format!("Controller mode\n\n{LOOKING}"));
        loop {
            let host = match find() {
                Ok(host) => host,
                Err(why) => {
                    status(&format!("Controller mode\n\n{why}"));
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            };
            eprintln!("[phone] found {} at {}", host.name, host.addr);
            status(&format!("Controller mode\n\nConnecting to {}…", host.name));
            let mut heard = Instant::now();
            let mut player = None;
            while heard.elapsed() < TIMEOUT {
                crate::gfx::wait_foreground();
                let (buttons, stick) = (crate::input::TOUCH_BUTTONS.load(Relaxed), crate::input::TOUCH_STICK.load(Relaxed));
                let (lx, ly) = (f32::from_bits((stick >> 32) as u32), f32::from_bits(stick as u32));
                let pad = Pad { buttons, sticks: [lx, ly, 0.0, 0.0], motion: crate::input::phone_motion_now(&mut motion) };
                let _ = sock.send_to(&pad.encode(), host.addr);
                let mut buf = [0u8; 16];
                while let Ok((n, from)) = sock.recv_from(&mut buf) {
                    if from == host.addr && n >= 5 && buf[..4] == ACK {
                        heard = Instant::now();
                        if player != Some(buf[4]) {
                            player = Some(buf[4]);
                            status(&match buf[4] {
                                0 => format!("Controller mode\n\n{} has no free player slot", host.name),
                                n => format!("Player {n} on {}", host.name),
                            });
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(8));
            }
            eprintln!("[phone] lost {}: no answer for {} s", host.name, TIMEOUT.as_secs());
        }
    }

    const LOOKING: &str = "Looking for Mush8 running on this Wi-Fi…\n(start the game there, with Remote controllers on)";
    const DENIED: &str = "Local network access is off for Mush8\n\nTurn it on in Settings → Privacy & Security → Local Network";

    #[derive(Default)]
    struct Browse {
        seen: Option<(CString, CString, CString, u32)>,
        err: i32,
    }

    fn find() -> Result<Host, String> {
        let typ = CString::new(TYPE).unwrap();
        let mut b = Browse::default();
        unsafe {
            let mut sd: DNSServiceRef = std::ptr::null_mut();
            let ctx = &mut b as *mut Browse as *mut c_void;
            let r = DNSServiceBrowse(&mut sd, 0, 0, typ.as_ptr(), std::ptr::null(), Some(on_browse), ctx);
            if r != 0 {
                eprintln!("[phone] bonjour browse failed ({r}){}", denied(r));
                return Err(if r == POLICY_DENIED { DENIED.into() } else { format!("Bonjour is unavailable ({r})") });
            }
            let start = Instant::now();
            while b.seen.is_none() && b.err == 0 && start.elapsed() < Duration::from_secs(3) {
                if wait(sd, 500) && DNSServiceProcessResult(sd) != 0 {
                    break;
                }
            }
            DNSServiceRefDeallocate(sd);
        }
        if b.err != 0 {
            eprintln!("[phone] bonjour browse error {}{}", b.err, denied(b.err));
            return Err(if b.err == POLICY_DENIED { DENIED.into() } else { format!("Bonjour error {}", b.err) });
        }
        let Some((name, regtype, domain, iface)) = b.seen else { return Err(LOOKING.into()) };
        let shown = name.to_string_lossy().into_owned();
        let Some(r) = resolve(&name, &regtype, &domain, iface) else {
            eprintln!("[phone] found {shown} but could not resolve its address");
            return Err(format!("Found {shown} but couldn't reach it\n\nCheck both are on the same Wi-Fi"));
        };
        Ok(Host { name: shown, addr: SocketAddr::from((r.ip, r.port)) })
    }

    unsafe extern "C" fn on_browse(_: DNSServiceRef, flags: u32, iface: u32, err: i32, name: *const c_char, regtype: *const c_char, domain: *const c_char, ctx: *mut c_void) {
        let b = unsafe { &mut *(ctx as *mut Browse) };
        if err != 0 {
            b.err = err;
            return;
        }
        if flags & ADD == 0 || name.is_null() || regtype.is_null() || domain.is_null() || b.seen.is_some() {
            return;
        }
        b.seen = unsafe { Some((CStr::from_ptr(name).to_owned(), CStr::from_ptr(regtype).to_owned(), CStr::from_ptr(domain).to_owned(), iface)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_roundtrip() {
        let p = Pad { buttons: 0x8421, sticks: [0.5, -1.0, 0.25, 0.0], motion: Some(Motion { accel: [0.1, -0.98, 0.2], gyro: [1.5, -2.0, 0.3] }) };
        let q = Pad::decode(&p.encode()).unwrap();
        assert_eq!(q.buttons, p.buttons);
        assert_eq!(q.sticks, p.sticks);
        let (m, n) = (p.motion.unwrap(), q.motion.unwrap());
        assert_eq!((m.accel, m.gyro), (n.accel, n.gyro));
        assert!(Pad::decode(&Pad { motion: None, ..p }.encode()).unwrap().motion.is_none());
        assert!(Pad::decode(b"nope").is_none());
    }

    #[test]
    fn one_entry_per_device() {
        let pad = Pad::default();
        let at = |s: &str| s.parse::<SocketAddr>().unwrap();
        let (safari, app, other) = (at("10.9.9.1:50001"), at("10.9.9.1:50002"), at("10.9.9.2:50003"));
        let (phone, second) = (device_id(safari), device_id(other));
        let entries = |d: u64| PHONES.lock().unwrap().iter().filter(|p| p.id == d).map(|p| p.addr).collect::<Vec<_>>();
        assert_eq!(phone, device_id(app));
        assert_ne!(phone, second);
        assert!(feed(safari, pad) && feed(other, pad));
        set_player(phone, 1);
        assert!(feed(app, pad));
        assert_eq!(entries(phone), vec![app]);
        assert_eq!(player(app), Some(1));
        assert!(!feed(safari, pad));
        assert_eq!(entries(phone), vec![app]);
        assert_eq!(entries(second), vec![other]);
        assert!(is_web(phone) && is_web(second));
        forget(safari);
        assert!(feed(safari, pad));
        assert_eq!(entries(phone), vec![safari]);
        for a in [safari, app, other] {
            forget(a);
        }
        assert!(entries(phone).is_empty() && entries(second).is_empty());
    }
}
