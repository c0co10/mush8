// Mush8 - mod.rs
// Constantin Clerc (c22dev) - 0.1
mod dsp;
mod effects;
pub mod output;
mod route;
mod system;

use dsp::Biquad;

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub sample_rate: u32,
    pub sample_count: u32,
    pub mix_buffer_count: u32,
    pub sub_mix_count: u32,
    pub voice_count: u32,
    pub sink_count: u32,
    pub effect_count: u32,
    pub revision: u32,
}

impl Config {
    pub fn read(d: &[u8]) -> Self {
        let u = |o: usize| d.get(o..o + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
        Self {
            sample_rate: u(0),
            sample_count: u(4),
            mix_buffer_count: u(8),
            sub_mix_count: u(0xC),
            voice_count: u(0x10),
            sink_count: u(0x14),
            effect_count: u(0x18),
            revision: u(0x30),
        }
    }
}

fn revision(magic: u32) -> u32 {
    magic.wrapping_sub(u32::from_le_bytes(*b"REV0")) >> 24
}

const UNUSED_MIX: i32 = i32::MAX;
const UNUSED_SPLITTER: u32 = u32::MAX;
const MIX_BUFFERS_MAX: usize = 24;
const VOICE_CHANNELS_MAX: usize = 6;
fn depop_decay(rate: u32) -> f32 {
    if rate == 48000 { 0.962189 } else { 0.943695 }
}

const IN_HEADER: usize = 0x40;
const IN_POOL: usize = 0x20;
const IN_CHANNEL: usize = 0x70;
const IN_VOICE: usize = 0x170;
const IN_EFFECT: usize = 0xC0;
const IN_MIX: usize = 0x930;
const IN_SINK: usize = 0x140;
const OUT_POOL: usize = 0x10;
const OUT_VOICE: usize = 0x10;
const OUT_EFFECT: usize = 0x10;
const OUT_SINK: usize = 0x20;
const OUT_PERFORMANCE: usize = 0x10;
const OUT_BEHAVIOR: usize = 0xB0;
const OUT_RENDER_INFO: usize = 0x10;

const POOL_REQUEST_DETACH: u32 = 2;
const POOL_DETACHED: u32 = 3;
const POOL_REQUEST_ATTACH: u32 = 4;
const POOL_ATTACHED: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    PcmInt16,
    PcmFloat,
    Adpcm,
}

impl SampleFormat {
    fn from(v: u8) -> Option<Self> {
        match v {
            2 => Some(Self::PcmInt16),
            5 => Some(Self::PcmFloat),
            6 => Some(Self::Adpcm),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PlayState {
    Started,
    #[default]
    Stopped,
    Stopping,
    Paused,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WaveBuffer {
    addr: u64,
    size: u64,
    context: u64,
    start: u32,
    end: u32,
    looping: bool,
    end_of_stream: bool,
    sent: bool,
    loop_start: u32,
    loop_end: u32,
    loop_count: i32,
}

impl WaveBuffer {
    fn range(&self, loops: i32) -> (usize, usize) {
        if loops > 0 && self.loop_start != 0 && self.loop_end != 0 && self.loop_start <= self.loop_end {
            (self.loop_start as usize, self.loop_end as usize)
        } else {
            (self.start as usize, self.end as usize)
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct VoiceState {
    biquad: [[f32; 2]; 2],
    played_samples: u64,
    offset: u32,
    wave_buffer_index: u32,
    valid: [bool; 4],
    consumed: u32,
    history: [i16; dsp::TAPS],
    fraction: f32,
    adpcm: [i16; 3],
    last: [f32; MIX_BUFFERS_MAX],
    loop_count: i32,
}

impl VoiceState {
    fn end_wave_buffer(&mut self, wb: &WaveBuffer) {
        self.valid[self.wave_buffer_index as usize] = false;
        self.wave_buffer_index = (self.wave_buffer_index + 1) % 4;
        self.loop_count = 0;
        self.consumed += 1;
        if wb.end_of_stream {
            self.played_samples = 0;
        }
    }
}

#[derive(Default)]
struct Channel {
    mix: [f32; MIX_BUFFERS_MAX],
    prev_mix: [f32; MIX_BUFFERS_MAX],
    state: VoiceState,
}

#[derive(Default)]
struct Voice {
    in_use: bool,
    is_new: bool,
    was_playing: bool,
    format: Option<SampleFormat>,
    sample_rate: u32,
    channels: usize,
    mix_id: i32,
    splitter_id: u32,
    play_state: PlayState,
    prev_play_state: PlayState,
    pitch: f32,
    volume: f32,
    prev_volume: f32,
    biquads: [Biquad; 2],
    biquad_was_enabled: [bool; 2],
    wave_buffer_count: u32,
    wave_buffer_index: u32,
    behaviour: u16,
    data_source: (u64, u64),
    data_source_unmapped: bool,
    buffer_unmapped: bool,
    flush_count: u8,
    wave_buffers: [WaveBuffer; 4],
    channel_ids: [usize; VOICE_CHANNELS_MAX],
}

impl Voice {
    fn reset(&mut self) {
        *self = Voice { mix_id: UNUSED_MIX, splitter_id: UNUSED_SPLITTER, ..Default::default() };
        for wb in &mut self.wave_buffers {
            wb.sent = true;
        }
    }

    fn has_destination(&self) -> bool {
        self.mix_id != UNUSED_MIX || self.splitter_id != UNUSED_SPLITTER
    }

    fn skip(&self) -> bool {
        !self.in_use || self.wave_buffer_count == 0 || self.data_source_unmapped || self.buffer_unmapped
    }

    fn set_play_state(&mut self, user: u8) {
        let old = self.play_state;
        self.prev_play_state = old;
        self.play_state = match user {
            0 => PlayState::Started,
            1 if old == PlayState::Stopped => return,
            1 => PlayState::Stopping,
            _ => PlayState::Paused,
        };
    }
}

#[derive(Default, Clone)]
struct Mix {
    used: bool,
    mix_id: i32,
    volume: f32,
    sample_rate: u32,
    buffer_count: usize,
    buffer_offset: usize,
    volumes: Vec<f32>,
    dest_mix: i32,
    distance: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Usage {
    #[default]
    Invalid,
    New,
    Enabled,
    Disabled,
}

#[derive(Default)]
struct Effect {
    kind: u8,
    enabled: bool,
    mix_id: i32,
    order: u32,
    usage: Usage,
    params: Vec<u8>,
    buffer_unmapped: bool,
    delay: Option<effects::Delay>,
}

const EFFECT_BUFFER_MIX: u8 = 1;
const EFFECT_AUX: u8 = 2;
const EFFECT_DELAY: u8 = 3;

#[derive(Default)]
struct Sink {
    used: bool,
    device: bool,
    inputs: Vec<u8>,
}

const SINK_CENTER: usize = 2;
const SINK_SCALE: f32 = 1.0 / 32768.0;

pub struct Renderer {
    cfg: Config,
    rev: u32,
    force_map: bool,
    pools: Vec<(u64, u64)>,
    channels: Vec<Channel>,
    voices: Vec<Voice>,
    mixes: Vec<Mix>,
    mix_order: Vec<usize>,
    effects: Vec<Effect>,
    sinks: Vec<Sink>,
    buffers: Vec<f32>,
    depop: Vec<f32>,
    scratch: Vec<i16>,
    elapsed_frames: u64,
}

fn rd<const N: usize>(d: &[u8], o: usize) -> [u8; N] {
    d.get(o..o + N).map_or([0; N], |b| b.try_into().unwrap())
}
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(rd(d, o))
}
fn i32_at(d: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(rd(d, o))
}
fn u64_at(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(rd(d, o))
}
fn f32_at(d: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(rd(d, o))
}

fn mapped(pools: &[(u64, u64)], force: bool, addr: u64, size: u64) -> bool {
    addr != 0 && (force || pools.iter().any(|&(a, s)| s != 0 && addr >= a && addr + size <= a + s))
}

impl Renderer {
    pub fn new(cfg: Config) -> Self {
        let n = cfg.sample_count as usize;
        let mixes = cfg.sub_mix_count as usize + 1;
        let mut voices: Vec<Voice> = (0..cfg.voice_count).map(|_| Voice::default()).collect();
        voices.iter_mut().for_each(Voice::reset);
        Renderer {
            cfg,
            rev: revision(cfg.revision),
            force_map: false,
            pools: Vec::new(),
            channels: (0..cfg.voice_count).map(|_| Channel::default()).collect(),
            voices,
            mixes: vec![Mix { mix_id: UNUSED_MIX, dest_mix: UNUSED_MIX, volumes: vec![0.0; MIX_BUFFERS_MAX * MIX_BUFFERS_MAX], ..Default::default() }; mixes],
            mix_order: (0..mixes).collect(),
            effects: (0..cfg.effect_count).map(|_| Effect::default()).collect(),
            sinks: (0..cfg.sink_count).map(|_| Sink::default()).collect(),
            buffers: vec![0.0; (cfg.mix_buffer_count as usize + VOICE_CHANNELS_MAX) * n],
            depop: vec![0.0; cfg.mix_buffer_count as usize],
            scratch: vec![0; dsp::SCRATCH],
            elapsed_frames: 0,
        }
    }

    fn mapped(&self, addr: u64, size: u64) -> bool {
        mapped(&self.pools, self.force_map, addr, size)
    }

    pub fn update(&mut self, input: &[u8], out: &mut [u8], active: bool) {
        out.fill(0);
        if input.len() < IN_HEADER || revision(u32_at(input, 0)) != self.rev {
            return;
        }
        let h = |i: usize| u32_at(input, i * 4) as usize;
        let (behavior, pools_size, channels_size, voices_size, effects_size, mixes_size, sinks_size, splitter_size) =
            (h(1), h(2), h(4), h(3), h(5), h(6), h(7), h(9));
        let mut at = IN_HEADER;
        let mut section = |size: usize| {
            let s = input.get(at..at + size).unwrap_or(&[]);
            at += size;
            s
        };
        self.force_map = u64_at(section(behavior), 8) & 1 != 0;
        let pools_in = section(pools_size);
        let channels_in = section(channels_size);
        let voices_in = section(voices_size);
        let effects_in = section(effects_size);
        if self.rev >= 2 {
            section(splitter_size);
        }
        let mixes_in = section(mixes_size);
        let sinks_in = section(sinks_size);

        let mut o = Vec::with_capacity(out.len());
        o.resize(IN_HEADER, 0);
        fn put(o: &mut [u8], i: usize, v: usize) {
            o[i * 4..i * 4 + 4].copy_from_slice(&(v as u32).to_le_bytes());
        }
        put(&mut o, 0, u32_at(input, 0) as usize);

        self.pools.resize(pools_in.len() / IN_POOL, (0, 0));
        for (i, p) in pools_in.chunks_exact(IN_POOL).enumerate() {
            let (addr, size, state) = (u64_at(p, 0), u64_at(p, 8), u32_at(p, 0x10));
            let out_state = match state {
                POOL_REQUEST_ATTACH => {
                    self.pools[i] = (addr, size);
                    POOL_ATTACHED
                }
                POOL_REQUEST_DETACH => {
                    self.pools[i] = (0, 0);
                    POOL_DETACHED
                }
                _ => 0,
            };
            o.extend_from_slice(&out_state.to_le_bytes());
            o.resize(o.len() + OUT_POOL - 4, 0);
        }
        put(&mut o, 2, self.pools.len() * OUT_POOL);

        for (c, p) in self.channels.iter_mut().zip(channels_in.chunks_exact(IN_CHANNEL)) {
            for (k, m) in c.mix.iter_mut().enumerate() {
                *m = f32_at(p, 4 + k * 4);
            }
        }

        for v in &mut self.voices {
            v.in_use = false;
        }
        for i in 0..self.voices.len() {
            let status = voices_in.get(i * IN_VOICE..(i + 1) * IN_VOICE).filter(|p| p[9] != 0).map(|p| self.update_voice(i, p));
            o.extend_from_slice(&status.unwrap_or([0; OUT_VOICE]));
        }
        put(&mut o, 3, self.voices.len() * OUT_VOICE);

        for (e, p) in self.effects.iter_mut().zip(effects_in.chunks_exact(IN_EFFECT)) {
            let (kind, is_new, enabled) = (p[0], p[1] != 0, p[2] != 0);
            if kind != e.kind {
                *e = Effect { kind, ..Default::default() };
            }
            e.enabled = enabled;
            e.mix_id = i32_at(p, 4);
            e.order = u32_at(p, 0x18);
            e.params = p[0x20..].to_vec();
            let (base, size) = (u64_at(p, 8), u64_at(p, 0x10));
            e.buffer_unmapped = if kind == EFFECT_AUX {
                let size = effects::Aux::ring_size(&e.params);
                !effects::Aux::rings(&e.params).iter().all(|&ring| mapped(&self.pools, self.force_map, ring, size))
            } else {
                size != 0 && !mapped(&self.pools, self.force_map, base, size)
            };
            if is_new || e.usage == Usage::Invalid {
                e.usage = Usage::New;
                e.delay = None;
            }
            let disabled = if active { e.usage == Usage::Disabled } else { e.usage != Usage::New };
            let state: u8 = if disabled { 4 } else { 3 };
            o.push(state);
            o.resize(o.len() + OUT_EFFECT - 1, 0);
        }
        put(&mut o, 5, self.effects.len() * OUT_EFFECT);

        let (mixes_in, dirty_only) = if self.rev >= 7 { (mixes_in.get(0x20..).unwrap_or(&[]), true) } else { (mixes_in, false) };
        for (i, p) in mixes_in.chunks_exact(IN_MIX).enumerate() {
            let id = if dirty_only { i32_at(p, 0x10) as usize } else { i };
            let Some(m) = self.mixes.get_mut(id) else { continue };
            m.used = p[0xC] != 0;
            if m.used {
                m.volume = f32_at(p, 0);
                m.sample_rate = u32_at(p, 4);
                m.buffer_count = (u32_at(p, 8) as usize).min(MIX_BUFFERS_MAX);
                m.mix_id = i32_at(p, 0x10);
                for (k, v) in m.volumes.iter_mut().enumerate() {
                    *v = f32_at(p, 0x24 + k * 4);
                }
                m.dest_mix = i32_at(p, 0x924);
            }
        }
        self.sort_mixes();

        for (s, p) in self.sinks.iter_mut().zip(sinks_in.chunks_exact(IN_SINK)) {
            let d = &p[0x20..];
            s.used = p[1] != 0;
            s.device = p[0] == 1;
            let n = (u32_at(d, 0x100) as usize).min(VOICE_CHANNELS_MAX);
            s.inputs = d[0x104..0x104 + n].to_vec();
        }
        o.resize(o.len() + self.sinks.len() * OUT_SINK, 0);
        put(&mut o, 7, self.sinks.len() * OUT_SINK);

        o.resize(o.len() + OUT_PERFORMANCE + OUT_BEHAVIOR, 0);
        put(&mut o, 8, OUT_PERFORMANCE);
        put(&mut o, 1, OUT_BEHAVIOR);
        if self.rev >= 5 {
            o.extend_from_slice(&self.elapsed_frames.to_le_bytes());
            o.resize(o.len() + OUT_RENDER_INFO - 8, 0);
            put(&mut o, 10, OUT_RENDER_INFO);
        }
        let total = o.len();
        put(&mut o, 15, total);
        let n = o.len().min(out.len());
        out[..n].copy_from_slice(&o[..n]);
    }

    fn update_voice(&mut self, i: usize, p: &[u8]) -> [u8; OUT_VOICE] {
        let rev = self.rev;
        let (data_source, size) = (u64_at(p, 0x48), u64_at(p, 0x50));
        let mut wave_buffers = [(false, WaveBuffer::default(), 0u64); 4];
        for (k, w) in wave_buffers.iter_mut().enumerate() {
            let b = &p[0x60 + k * 0x38..];
            let wb = WaveBuffer {
                addr: u64_at(b, 0),
                size: u64_at(b, 8),
                start: u32_at(b, 0x10),
                end: u32_at(b, 0x14),
                looping: b[0x18] != 0,
                end_of_stream: b[0x19] != 0,
                loop_count: if rev >= 8 { i32_at(b, 0x1C) } else { -1 },
                context: u64_at(b, 0x20),
                loop_start: if rev >= 8 { u32_at(b, 0x30) } else { 0 },
                loop_end: if rev >= 8 { u32_at(b, 0x34) } else { 0 },
                sent: false,
            };
            *w = (b[0x1A] != 0, wb, u64_at(b, 0x28));
        }
        let format = SampleFormat::from(p[0xB]);
        let valid_offsets = |wb: &WaveBuffer| {
            let fits = |s: u32| match format {
                Some(SampleFormat::PcmInt16) => s as u64 * 2 <= wb.size,
                Some(SampleFormat::PcmFloat) => s as u64 * 4 <= wb.size,
                Some(SampleFormat::Adpcm) => dsp::adpcm_size(s) <= wb.size,
                None => false,
            };
            fits(wb.start) && fits(wb.end)
        };
        let mapped: Vec<_> = wave_buffers.iter().map(|(_, wb, ctx_size)| (self.mapped(wb.addr, wb.size), wb.context == 0 || self.mapped(wb.context, *ctx_size))).collect();
        let source_mapped = self.mapped(data_source, size);

        let v = &mut self.voices[i];
        if p[8] != 0 {
            v.reset();
        }
        v.in_use = true;
        v.set_play_state(p[0xA]);
        v.format = format;
        v.sample_rate = u32_at(p, 0xC);
        v.channels = (u32_at(p, 0x18) as usize).min(VOICE_CHANNELS_MAX);
        v.pitch = f32_at(p, 0x1C);
        v.volume = f32_at(p, 0x20);
        v.biquads = [Biquad::read(&p[0x24..]), Biquad::read(&p[0x30..])];
        v.wave_buffer_count = u32_at(p, 0x3C);
        v.wave_buffer_index = u32_at(p, 0x40);
        v.mix_id = i32_at(p, 0x58);
        v.splitter_id = if rev >= 2 { u32_at(p, 0x5C) } else { UNUSED_SPLITTER };
        for (k, id) in v.channel_ids.iter_mut().enumerate() {
            *id = (i32_at(p, 0x140 + k * 4).max(0) as usize).min(self.channels.len().saturating_sub(1));
        }
        if rev >= 5 {
            v.flush_count = v.flush_count.wrapping_add(p[0x159]);
            v.behaviour = u16::from_le_bytes(rd(p, 0x15C));
        }
        if v.data_source != (data_source, size) {
            v.data_source = (data_source, size);
            v.data_source_unmapped = !source_mapped;
        }
        for (k, (sent_to_server, wb, _)) in wave_buffers.into_iter().enumerate() {
            if (!sent_to_server || v.buffer_unmapped) && valid_offsets(&wb) {
                let (buffer_ok, context_ok) = mapped[k];
                let adpcm_context = format == Some(SampleFormat::Adpcm) && rev >= 2 && wb.context != 0;
                v.wave_buffers[k] = WaveBuffer { context: if adpcm_context { wb.context } else { 0 }, ..wb };
                v.buffer_unmapped = !buffer_ok || (adpcm_context && !context_ok);
            }
        }

        let mut status = [0u8; OUT_VOICE];
        if p[8] != 0 || v.is_new {
            v.is_new = true;
        } else {
            let st = &self.channels[v.channel_ids[0]].state;
            status[..8].copy_from_slice(&st.played_samples.to_le_bytes());
            status[8..12].copy_from_slice(&st.consumed.to_le_bytes());
        }
        status
    }

    fn sort_mixes(&mut self) {
        let n = self.mixes.len();
        for m in &mut self.mixes {
            m.distance = u32::MAX;
        }
        for i in 0..n {
            if !self.mixes[i].used || self.mixes[i].mix_id == 0 {
                continue;
            }
            let mut id = self.mixes[i].mix_id;
            let mut distance = 0;
            while distance < n as u32 {
                let Some(m) = (id != UNUSED_MIX).then(|| self.mixes.get(id as usize)).flatten() else {
                    distance = u32::MAX;
                    break;
                };
                if m.distance != u32::MAX {
                    distance = m.distance + 1;
                    break;
                }
                id = m.dest_mix;
                if id == 0 {
                    break;
                }
                distance += 1;
            }
            self.mixes[i].distance = if distance > n as u32 { u32::MAX } else { distance };
        }
        self.mix_order = (0..n).collect();
        self.mix_order.sort_by_key(|&i| std::cmp::Reverse(self.mixes[i].distance));
        let mut offset = 0;
        for m in &mut self.mixes {
            m.buffer_offset = offset;
            offset += m.buffer_count;
        }
    }

    fn buffer(&mut self, i: usize) -> &mut [f32] {
        let n = self.cfg.sample_count as usize;
        &mut self.buffers[i * n..(i + 1) * n]
    }

    fn buffer_pair(&mut self, i: usize, o: usize) -> (&[f32], &mut [f32]) {
        let n = self.cfg.sample_count as usize;
        if i < o {
            let (a, b) = self.buffers.split_at_mut(o * n);
            (&a[i * n..(i + 1) * n], &mut b[..n])
        } else {
            let (a, b) = self.buffers.split_at_mut(i * n);
            (&b[..n], &mut a[o * n..(o + 1) * n])
        }
    }

    pub fn render(&mut self, out: &mut [f32]) {
        self.buffers.fill(0.0);
        for i in 0..self.voices.len() {
            if !self.voices[i].skip() && self.prepare_voice(i) {
                self.render_voice(i);
            }
        }
        for k in 0..self.mix_order.len() {
            let i = self.mix_order[k];
            if i != 0 && self.mixes[i].used {
                self.render_mix(i);
            }
        }
        self.render_mix(0);
        self.render_sink(out);
        self.elapsed_frames += 1;
    }

    fn prepare_voice(&mut self, i: usize) -> bool {
        let v = &mut self.voices[i];
        let ids = v.channel_ids;
        let chans = &mut self.channels;
        if v.is_new {
            for &id in &ids[..v.channels] {
                chans[id].state = VoiceState::default();
                chans[id].prev_mix = chans[id].mix;
            }
            v.prev_volume = v.volume;
            v.is_new = false;
        }
        if v.flush_count != 0 {
            let mut k = v.wave_buffer_index as usize % 4;
            for _ in 0..v.flush_count {
                v.wave_buffers[k].sent = true;
                for &id in &ids[..v.channels] {
                    let st = &mut chans[id].state;
                    if st.valid[k] {
                        st.wave_buffer_index = (st.wave_buffer_index + 1) % 4;
                        st.consumed += 1;
                        st.valid[k] = false;
                    }
                }
                k = (k + 1) % 4;
            }
            v.flush_count = 0;
        }
        match v.play_state {
            PlayState::Started => {
                for (k, wb) in v.wave_buffers.iter_mut().enumerate() {
                    if !wb.sent {
                        for &id in &ids[..v.channels] {
                            chans[id].state.valid[k] = true;
                        }
                        wb.sent = true;
                    }
                }
                v.was_playing = false;
                chans[ids[0]].state.valid.iter().any(|&b| b)
            }
            PlayState::Stopping => {
                for (k, wb) in v.wave_buffers.iter_mut().enumerate() {
                    wb.sent = true;
                    for &id in &ids[..v.channels] {
                        let st = &mut chans[id].state;
                        if st.valid[k] {
                            st.wave_buffer_index = (st.wave_buffer_index + 1) % 4;
                            st.consumed += 1;
                        }
                        st.valid[k] = false;
                    }
                }
                for &id in &ids[..v.channels] {
                    let st = &mut chans[id].state;
                    (st.offset, st.played_samples, st.history, st.fraction, st.adpcm) = Default::default();
                }
                v.play_state = PlayState::Stopped;
                v.was_playing = v.prev_play_state == PlayState::Started;
                v.was_playing
            }
            PlayState::Stopped | PlayState::Paused => {
                v.was_playing = v.prev_play_state == PlayState::Started;
                v.was_playing
            }
        }
    }

    fn render_voice(&mut self, i: usize) {
        let base = self.cfg.mix_buffer_count as usize;
        let rate = self.cfg.sample_rate;
        let v = &self.voices[i];
        let mix = (v.mix_id != UNUSED_MIX).then(|| self.mixes.get(v.mix_id as usize)).flatten().map(|m| (m.buffer_offset, m.buffer_count));
        let Some(format) = v.format else { return };
        let mut coefs = [0i16; 16];
        if format == SampleFormat::Adpcm && v.data_source.0 != 0 {
            let n = (v.data_source.1 as usize / 2).min(16);
            let c = unsafe { std::slice::from_raw_parts(v.data_source.0 as *const i16, n) };
            coefs[..n].copy_from_slice(c);
        }
        let (was_playing, ids, channels, volumes) = (v.was_playing, v.channel_ids, v.channels, (v.prev_volume, v.volume));
        let biquads = v.biquads;
        let wave_buffers = v.wave_buffers;
        for ch in 0..channels {
            let id = ids[ch];
            let n = self.cfg.sample_count as usize;
            if let Some((offset, count)) = mix.filter(|_| was_playing) {
                let st = &mut self.channels[id].state;
                for k in 0..count {
                    if st.last[k] != 0.0 && offset + k < self.depop.len() {
                        self.depop[offset + k] += st.last[k];
                        st.last[k] = 0.0;
                    }
                }
            }
            if was_playing {
                continue;
            }
            let src = dsp::Source { format, sample_rate: self.voices[i].sample_rate, pitch: self.voices[i].pitch, channel: ch, channels, behaviour: self.voices[i].behaviour, coefs, wave_buffers: &wave_buffers };
            let mut state = self.channels[id].state;
            let mut scratch = std::mem::take(&mut self.scratch);
            dsp::process_wave_buffers(self.buffer(base + ch), &src, &mut state, rate, &mut scratch);
            self.scratch = scratch;
            let Some((offset, count)) = mix else {
                self.channels[id].state = state;
                continue;
            };
            let was_enabled = self.voices[i].biquad_was_enabled;
            for (k, f) in biquads.iter().enumerate() {
                if f.enable {
                    if !was_enabled[k] {
                        state.biquad[k] = [0.0; 2];
                    }
                    f.process(&mut state.biquad[k], &mut self.buffers[(base + ch) * n..(base + ch + 1) * n]);
                }
            }
            let (v0, v1) = volumes;
            let ramp = (v1 - v0) / n as f32;
            for (k, s) in self.buffer(base + ch).iter_mut().enumerate() {
                *s *= v0 + ramp * k as f32;
            }
            let (mix_vol, prev_mix) = (self.channels[id].mix, self.channels[id].prev_mix);
            for k in 0..count.min(base.saturating_sub(offset)) {
                let (m0, m1) = (prev_mix[k], mix_vol[k]);
                if m0 == 0.0 && m1 == 0.0 {
                    continue;
                }
                let ramp = (m1 - m0) / n as f32;
                let (input, output) = self.buffer_pair(base + ch, offset + k);
                let mut last = 0.0;
                for (j, (o, s)) in output.iter_mut().zip(input).enumerate() {
                    last = s * (m0 + ramp * j as f32);
                    *o += last;
                }
                state.last[k] = last;
            }
            self.channels[id].prev_mix = mix_vol;
            self.channels[id].state = state;
        }
        let v = &mut self.voices[i];
        if was_playing {
            v.prev_volume = 0.0;
        } else if v.has_destination() {
            v.prev_volume = v.volume;
            v.biquad_was_enabled = [biquads[0].enable, biquads[1].enable];
        }
    }

    fn render_mix(&mut self, i: usize) {
        let total = self.cfg.mix_buffer_count as usize;
        let m = self.mixes[i].clone();
        let decay = depop_decay(m.sample_rate);
        for b in m.buffer_offset..(m.buffer_offset + m.buffer_count).min(total) {
            let mut d = self.depop[b];
            if d != 0.0 {
                for s in self.buffer(b) {
                    d *= decay;
                    *s += d;
                }
                self.depop[b] = d;
            }
        }
        let mut effects: Vec<usize> = (0..self.effects.len()).filter(|&e| self.effects[e].kind != 0 && self.effects[e].mix_id == m.mix_id).collect();
        effects.sort_by_key(|&e| self.effects[e].order);
        for e in effects {
            self.render_effect(e, m.buffer_offset);
        }
        if i == 0 {
            if m.volume != 1.0 {
                for b in m.buffer_offset..(m.buffer_offset + m.buffer_count).min(total) {
                    self.buffer(b).iter_mut().for_each(|s| *s *= m.volume);
                }
            }
            return;
        }
        let Some(dest) = (m.dest_mix != UNUSED_MIX).then(|| self.mixes.get(m.dest_mix as usize)).flatten().map(|d| (d.buffer_offset, d.buffer_count)) else { return };
        for b in 0..m.buffer_count {
            for d in 0..dest.1 {
                let vol = m.volume * m.volumes[b * MIX_BUFFERS_MAX + d];
                let (src, dst) = (m.buffer_offset + b, dest.0 + d);
                if vol != 0.0 && src < total && dst < total && src != dst {
                    let (input, output) = self.buffer_pair(src, dst);
                    output.iter_mut().zip(input).for_each(|(o, s)| *o += s * vol);
                }
            }
        }
    }

    fn render_effect(&mut self, e: usize, offset: usize) {
        let n = self.cfg.sample_count as usize;
        let total = self.cfg.mix_buffer_count as usize;
        let fx = &mut self.effects[e];
        if fx.buffer_unmapped {
            return;
        }
        fx.usage = if fx.enabled { Usage::Enabled } else { Usage::Disabled };
        let (kind, enabled, p) = (fx.kind, fx.enabled, fx.params.clone());
        let buf = |k: u8| Some(offset + k as usize).filter(|&b| b < total);
        match kind {
            EFFECT_BUFFER_MIX if enabled => {
                let count = (u32_at(&p, 0x90) as usize).min(MIX_BUFFERS_MAX);
                for k in 0..count {
                    let (Some(src), Some(dst), vol) = (buf(p[k]), buf(p[24 + k]), f32_at(&p, 0x30 + k * 4)) else { continue };
                    if vol != 0.0 && src != dst {
                        let (input, output) = self.buffer_pair(src, dst);
                        output.iter_mut().zip(input).for_each(|(o, s)| *o += s * vol);
                    }
                }
            }
            EFFECT_AUX => {
                let aux = effects::Aux::new(&p);
                let channels = (u32_at(&p, 0x30) as usize).min(MIX_BUFFERS_MAX);
                for k in 0..channels {
                    let (Some(src), Some(dst)) = (buf(p[k]), buf(p[24 + k])) else { continue };
                    let input = self.buffer(src).to_vec();
                    let output = self.buffer(dst);
                    if enabled {
                        let advance = if k + 1 == channels { (channels * n) as u32 } else { 0 };
                        aux.process(&input, output, (k * n) as u32, advance);
                    } else {
                        output.copy_from_slice(&input);
                    }
                }
                if !enabled {
                    aux.reset();
                }
            }
            EFFECT_DELAY => {
                let channels = effects::Delay::channels(&p).unwrap_or(0);
                let mut order: Vec<usize> = (0..channels).collect();
                if channels == 6 && self.rev < 11 {
                    order = vec![0, 1, 4, 5, 2, 3];
                }
                let (Some(ins), Some(outs)) = (order.iter().map(|&k| buf(p[k])).collect::<Option<Vec<_>>>(), order.iter().map(|&k| buf(p[6 + k])).collect::<Option<Vec<_>>>()) else { return };
                let mut io: Vec<Vec<f32>> = ins.iter().map(|&b| self.buffer(b).to_vec()).collect();
                if enabled {
                    let fx = &mut self.effects[e];
                    let mut delay = fx.delay.take().filter(|d| d.fits(&p)).unwrap_or_else(|| effects::Delay::new(&p));
                    delay.process(&p, &mut io);
                    fx.delay = Some(delay);
                }
                for (data, &b) in io.iter().zip(&outs) {
                    self.buffer(b).copy_from_slice(data);
                }
            }
            _ => {}
        }
    }

    fn render_sink(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let total = self.cfg.mix_buffer_count as usize;
        let base = self.mixes[0].buffer_offset;
        let Some(sink) = self.sinks.iter().find(|s| s.used && s.device && !s.inputs.is_empty()) else { return };
        let idx: Vec<usize> = sink.inputs.iter().map(|&k| base + k as usize).filter(|&b| b < total).collect();
        let n = self.cfg.sample_count as usize;
        let routing: &[(usize, usize)] = match idx.len() {
            0 => &[],
            1 => &[(SINK_CENTER, 0)],
            2..6 => &[(0, 0), (1, 1)],
            _ => &[(0, 0), (1, 1), (2, 2), (3, 3), (4, 4), (5, 5)],
        };
        for &(dst, src) in routing {
            let b = idx[src];
            for (o, s) in out[dst * n..(dst + 1) * n].iter_mut().zip(&self.buffers[b * n..(b + 1) * n]) {
                *o = (s * SINK_SCALE).clamp(-1.0, 1.0);
            }
        }
    }
}
