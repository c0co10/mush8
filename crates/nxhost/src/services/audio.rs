// Mush8 - audio.rs
// Constantin Clerc (c22dev) - 0.1
use crate::audio::{self, output::{self, Output}};
use crate::ipc::{Request, Response, Service, Stub, new_event, write_buffer};
use crate::kernel::{KERNEL, Obj};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

pub struct AudRen;

impl Service for AudRen {
    fn name(&self) -> String {
        "audren:u".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 => rsp.objects.push(Renderer::open(audio::Config::read(&req.data))),
            1 => rsp.push_u64(0x10_0000),
            2 | 4 => rsp.objects.push(Arc::new(AudioDevice)),
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

struct AudioDevice;

const DEVICE_NAME: &[u8] = b"AudioTvOutput";

fn device_name() -> Vec<u8> {
    let mut n = vec![0u8; 0x100];
    n[..DEVICE_NAME.len()].copy_from_slice(DEVICE_NAME);
    n
}

impl Service for AudioDevice {
    fn name(&self) -> String {
        "audio::IAudioDevice".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 | 6 => {
                let n = req.out_buffer().map_or(0, |b| write_buffer(&b, &device_name()) / 0x100);
                rsp.push_u32(n.min(1) as u32);
            }
            3 | 10 | 13 => {
                if let Some(b) = req.out_buffer() {
                    write_buffer(&b, &device_name());
                }
            }
            1 | 7 => {}
            2 | 8 => rsp.data.extend_from_slice(&1.0f32.to_le_bytes()),
            4 | 11 | 12 => rsp.copy_handles.push(new_event(false)),
            5 => rsp.push_u32(output::CHANNELS as u32),
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

struct Renderer {
    config: audio::Config,
    event: u32,
    started: Arc<AtomicBool>,
    mixer: Arc<Mutex<audio::Renderer>>,
    _output: Output,
}

impl Renderer {
    fn open(config: audio::Config) -> Arc<dyn Service> {
        eprintln!("[audio] renderer {config:?}");
        let event = new_event(false);
        let Some(Obj::ReadableEvent(signal)) = KERNEL.lock().get(event) else { unreachable!() };
        let started = Arc::new(AtomicBool::new(false));
        let mixer = Arc::new(Mutex::new(audio::Renderer::new(config)));
        let block = output::CHANNELS * config.sample_count as usize;
        let (s, m) = (started.clone(), mixer.clone());
        let output = output::start(
            config.sample_rate,
            config.sample_count as usize,
            Box::new(move |buf: &mut [f32]| {
                if !s.load(Relaxed) {
                    buf.fill(0.0);
                    return;
                }
                let mut m = m.lock().unwrap();
                for f in buf.chunks_exact_mut(block) {
                    m.render(f);
                }
                drop(m);
                crate::kernel::signal_event(&signal);
            }),
        );
        Arc::new(Renderer { config, event, started, mixer, _output: output })
    }
}

impl Service for Renderer {
    fn name(&self) -> String {
        "audio::IAudioRenderer".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 => rsp.push_u32(self.config.sample_rate),
            1 => rsp.push_u32(self.config.sample_count),
            2 => rsp.push_u32(self.config.mix_buffer_count),
            3 => rsp.push_u32(if self.started.load(Relaxed) { 0 } else { 1 }),
            4 | 10 => {
                let Some(inb) = req.in_buffer() else { return 0 };
                let input = unsafe { std::slice::from_raw_parts(inb.addr as *const u8, inb.size as usize) }.to_vec();
                if let Some(ob) = req.out_buffer() {
                    let mut buf = vec![0u8; ob.size as usize];
                    self.mixer.lock().unwrap().update(&input, &mut buf, self.started.load(Relaxed));
                    write_buffer(&ob, &buf);
                }
            }
            5 => self.started.store(true, Relaxed),
            6 => self.started.store(false, Relaxed),
            7 => rsp.copy_handles.push(self.event),
            8 | 11 => {}
            9 => rsp.push_u32(100),
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}
