// Mush8 - cmd.rs
// Constantin Clerc (c22dev) - 0.1
use super::draw;
use super::fx::FastMap;
use super::objects::STATE;
use crate::stubs::Args;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{LazyLock, Mutex};

pub type TexBuilder = [u8; 0x68];

#[derive(Debug, Clone, Copy)]
pub struct Draw {
    pub mode: u32,
    pub index: Option<(u32, u64)>,
    pub first: u32,
    pub count: u32,
    pub base_vertex: i32,
    pub base_instance: u32,
    pub instances: u32,
}

impl Draw {
    pub(super) fn validate_indices(&self) -> Result<(), &'static str> {
        let Some((ty, addr)) = self.index else { return Ok(()) };
        if self.count == 0 || self.instances == 0 {
            return Ok(());
        }
        if addr == 0 {
            return Err("null index buffer");
        }
        let size = match ty { 0 => 1, 1 => 2, _ => 4 };
        addr.checked_add(size * self.count as u64).ok_or("index buffer range overflows")?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum Cmd {
    SetRenderTargets { colors: Vec<Option<Box<TexBuilder>>>, depth: Option<Box<TexBuilder>> },
    ClearColor { index: u32, rgba: [f32; 4] },
    ClearDepthStencil { depth: f32, stencil: u32 },
    Call(Vec<u64>),
    Program(Vec<(u64, u64)>),
    VertexAttribs(Vec<u32>),
    VertexStreams(Vec<(u32, u32)>),
    VertexBuffer { index: u32, addr: u64, size: u64 },
    Uniform { stage: u32, index: u32, addr: u64, size: u64 },
    Texture { stage: u32, index: u32, handle: u64 },
    Storage { stage: u32, index: u32, addr: u64, size: u64 },
    UpdateUniform { addr: u64, size: u64, offset: u64, data: Vec<u8> },
    ColorState(u32),
    ChannelMask(u32),
    Blend(u64),
    DepthStencil(u64),
    Polygon(u32),
    Viewport([f32; 4]),
    Scissor([i32; 4]),
    BlendColor([f32; 4]),
    StencilRef(u32, u32),
    StencilValueMask(u32, u32),
    StencilMask(u32, u32),
    TexturePool(u64),
    SamplerPool(u64),
    DepthClamp(bool),
    CopyBufferToTexture { src: u64, dst: Box<TexBuilder>, region: [i32; 6] },
    CopyTextureToTexture { src: Box<TexBuilder>, src_region: [i32; 6], dst: Box<TexBuilder>, dst_region: [i32; 6], flags: u32 },
    CopyTextureToBuffer { src: Box<TexBuilder>, region: [i32; 6], dst: u64 },
    ClearTexture { dst: Box<TexBuilder>, region: [i32; 6], color: [f32; 4], mask: u32 },
    RenderEnable(bool),
    Dispatch([u32; 3]),
    RasterizerDiscard(bool),
    ReportCounter { counter: u32, addr: u64 },
    DepthRange(f32, f32),
    PolygonOffset([f32; 3]),
    Draw(Draw),
}

#[derive(Default)]
struct Recorder {
    open: FastMap<u64, Vec<Cmd>>,
    done: FastMap<u64, std::sync::Arc<Vec<Cmd>>>,
    regions: BTreeMap<u64, (u64, u64)>,
    untracked: VecDeque<u64>,
    control: FastMap<u64, u64>,
    next: u64,
    last_len: FastMap<u64, usize>,
}

static REC: LazyLock<Mutex<Recorder>> = LazyLock::new(|| Mutex::new(Recorder { next: 1, ..Default::default() }));
const MAX_UNTRACKED: usize = 8192;
const CMD_BYTES: u64 = 16;

pub fn begin(cb: u64) {
    REC.lock().unwrap().open.insert(cb, Vec::new());
}

pub fn add_control_memory(cb: u64, addr: u64) {
    REC.lock().unwrap().control.insert(cb, addr);
}

pub fn memory_used(cb: u64) -> u64 {
    let r = REC.lock().unwrap();
    r.open.get(&cb).map(|v| v.len()).or_else(|| r.last_len.get(&cb).copied()).unwrap_or(0) as u64 * CMD_BYTES
}

pub fn end(cb: u64) -> u64 {
    let mut guard = REC.lock().unwrap();
    let r = &mut *guard;
    let cmds = r.open.remove(&cb).unwrap_or_default();
    let mut dropped = Vec::new();
    r.last_len.insert(cb, cmds.len());
    let h = r.next;
    r.next += 1;
    let len = (cmds.len() as u64 * CMD_BYTES).max(1);
    r.done.insert(h, std::sync::Arc::new(cmds));
    if let Some(pos) = r.control.get_mut(&cb) {
        let (start, end) = (*pos, *pos + len);
        *pos = end;
        let dead: Vec<u64> = r.regions.range(..end).rev().take_while(|(_, (e, _))| *e > start).map(|(s, _)| *s).collect();
        for s in dead {
            if let Some((_, old)) = r.regions.remove(&s) {
                dropped.extend(r.done.remove(&old));
            }
        }
        r.regions.insert(start, (end, h));
    } else {
        r.untracked.push_back(h);
        if r.untracked.len() > MAX_UNTRACKED {
            let old = r.untracked.pop_front().unwrap();
            dropped.extend(r.done.remove(&old));
        }
    }
    drop(guard);
    reap(dropped);
    h
}

fn reap(old: Vec<std::sync::Arc<Vec<Cmd>>>) {
    static GARBAGE: Mutex<Vec<std::sync::Arc<Vec<Cmd>>>> = Mutex::new(Vec::new());
    static REAPER: std::sync::Once = std::sync::Once::new();
    if old.is_empty() {
        return;
    }
    REAPER.call_once(|| {
        std::thread::Builder::new()
            .name("nvn-reaper".into())
            .spawn(|| loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let batch = std::mem::take(&mut *GARBAGE.lock().unwrap());
                drop(batch);
            })
            .unwrap();
    });
    GARBAGE.lock().unwrap().extend(old);
}

pub fn recording(handle: u64) -> Option<std::sync::Arc<Vec<Cmd>>> {
    REC.lock().unwrap().done.get(&handle).cloned()
}

fn push(cb: u64, c: Cmd) {
    if let Some(v) = REC.lock().unwrap().open.get_mut(&cb) {
        v.push(c);
    }
}

fn rd<T: Copy>(addr: u64) -> T {
    unsafe { (addr as *const T).read_unaligned() }
}

fn rd_vec<T: Copy>(addr: u64, n: u64) -> Vec<T> {
    (0..n).map(|i| rd::<T>(addr + i * std::mem::size_of::<T>() as u64)).collect()
}

pub fn set_render_targets(a: &Args) -> u64 {
    let n = a.0[1] as usize;
    let objs: Vec<u64> = (0..n).map(|i| if a.0[2] == 0 { 0 } else { rd::<u64>(a.0[2] + 8 * i as u64) }).collect();
    let builder = |t: u64, view: u64| {
        super::objects::texture(t).map(|t| {
            let mut b = Box::new(t.builder);
            super::objects::apply_view(&mut b, view, true);
            b
        })
    };
    let views: Vec<u64> = (0..n).map(|i| if a.0[3] == 0 { 0 } else { rd::<u64>(a.0[3] + 8 * i as u64) }).collect();
    let colors: Vec<Option<Box<TexBuilder>>> = objs.iter().zip(&views).map(|(&t, &v)| builder(t, v)).collect();
    let depth = builder(a.0[4], a.0[5]);
    push(a.0[0], Cmd::SetRenderTargets { colors, depth });
    0
}

pub fn clear_color(a: &Args) -> u64 {
    let p = a.0[2];
    let rgba = [rd::<f32>(p), rd::<f32>(p + 4), rd::<f32>(p + 8), rd::<f32>(p + 12)];
    push(a.0[0], Cmd::ClearColor { index: a.0[1] as u32, rgba });
    0
}

pub fn clear_depth_stencil(a: &Args) -> u64 {
    push(a.0[0], Cmd::ClearDepthStencil { depth: a.f32(0), stencil: a.0[2] as u32 });
    0
}

pub fn bind_uniform_buffer(a: &Args) -> u64 {
    push(a.0[0], Cmd::Uniform { stage: a.0[1] as u32, index: a.0[2] as u32, addr: a.0[3], size: a.0[4] });
    0
}

pub fn bind_texture(a: &Args) -> u64 {
    push(a.0[0], Cmd::Texture { stage: a.0[1] as u32, index: a.0[2] as u32, handle: a.0[3] });
    0
}

pub fn call_commands(a: &Args) -> u64 {
    let handles = rd_vec::<u64>(a.0[2], a.0[1]);
    push(a.0[0], Cmd::Call(handles));
    0
}

pub fn bind_program(a: &Args) -> u64 {
    let stages = STATE.lock().unwrap().programs.get(&a.0[1]).cloned().unwrap_or_default();
    push(a.0[0], Cmd::Program(stages));
    0
}

pub fn bind_vertex_attribs(a: &Args) -> u64 {
    push(a.0[0], Cmd::VertexAttribs(rd_vec::<u32>(a.0[2], a.0[1].min(32))));
    0
}

pub fn bind_vertex_streams(a: &Args) -> u64 {
    push(a.0[0], Cmd::VertexStreams(rd_vec::<(u32, u32)>(a.0[2], a.0[1].min(32))));
    0
}

pub fn bind_vertex_buffer(a: &Args) -> u64 {
    push(a.0[0], Cmd::VertexBuffer { index: a.0[1] as u32, addr: a.0[2], size: a.0[3] });
    0
}

pub fn update_uniform_buffer(a: &Args) -> u64 {
    let len = a.0[4].min(0x10000) as usize;
    let data = unsafe { std::slice::from_raw_parts(a.0[5] as *const u8, len) }.to_vec();
    push(a.0[0], Cmd::UpdateUniform { addr: a.0[1], size: a.0[2], offset: a.0[3], data });
    0
}

pub fn host_impl(name: &str) -> Option<fn(&Args) -> u64> {
    Some(match name {
        "nvnCommandBufferBindProgram" => bind_program,
        "nvnCommandBufferBindVertexAttribState" => bind_vertex_attribs,
        "nvnCommandBufferBindVertexStreamState" => bind_vertex_streams,
        "nvnCommandBufferBindVertexBuffer" => bind_vertex_buffer,
        "nvnCommandBufferBindStorageBuffer" => |a| {
            push(a.0[0], Cmd::Storage { stage: a.0[1] as u32, index: a.0[2] as u32, addr: a.0[3], size: a.0[4] });
            0
        },
        "nvnCommandBufferUpdateUniformBuffer" => update_uniform_buffer,
        "nvnCommandBufferBindColorState" => |a| state_cmd(a, |p| Cmd::ColorState(rd(p))),
        "nvnCommandBufferBindChannelMaskState" => |a| state_cmd(a, |p| Cmd::ChannelMask(rd(p))),
        "nvnCommandBufferBindBlendState" => |a| state_cmd(a, |p| Cmd::Blend(rd(p))),
        "nvnCommandBufferBindDepthStencilState" => |a| state_cmd(a, |p| Cmd::DepthStencil(rd(p))),
        "nvnCommandBufferBindPolygonState" => |a| state_cmd(a, |p| Cmd::Polygon(rd(p))),
        "nvnCommandBufferSetBlendColor" => |a| state_cmd(a, |p| Cmd::BlendColor(rd(p))),
        "nvnCommandBufferSetTexturePool" => |a| state_cmd(a, Cmd::TexturePool),
        "nvnCommandBufferSetSamplerPool" => |a| state_cmd(a, Cmd::SamplerPool),
        "nvnCommandBufferSetViewport" => |a| {
            let v = [a.0[1] as i32 as f32, a.0[2] as i32 as f32, a.0[3] as i32 as f32, a.0[4] as i32 as f32];
            push(a.0[0], Cmd::Viewport(v));
            0
        },
        "nvnCommandBufferSetViewports" => |a| {
            if a.0[1] == 0 && a.0[2] > 0 {
                push(a.0[0], Cmd::Viewport(rd(a.0[3])));
            }
            0
        },
        "nvnCommandBufferSetScissor" => |a| {
            push(a.0[0], Cmd::Scissor([a.0[1] as i32, a.0[2] as i32, a.0[3] as i32, a.0[4] as i32]));
            0
        },
        "nvnCommandBufferSetScissors" => |a| {
            if a.0[1] == 0 && a.0[2] > 0 {
                push(a.0[0], Cmd::Scissor(rd(a.0[3])));
            }
            0
        },
        "nvnCommandBufferSetStencilRef" => |a| {
            push(a.0[0], Cmd::StencilRef(a.0[1] as u32, a.0[2] as u32));
            0
        },
        "nvnCommandBufferSetStencilValueMask" => |a| {
            push(a.0[0], Cmd::StencilValueMask(a.0[1] as u32, a.0[2] as u32));
            0
        },
        "nvnCommandBufferSetStencilMask" => |a| {
            push(a.0[0], Cmd::StencilMask(a.0[1] as u32, a.0[2] as u32));
            0
        },
        "nvnCommandBufferCopyBufferToTexture" => |a| {
            if let Some(b) = view_builder(a.0[2], a.0[3]) {
                push(a.0[0], Cmd::CopyBufferToTexture { src: a.0[1], dst: b, region: rd(a.0[4]) });
            }
            0
        },
        "nvnCommandBufferCopyTextureToTexture" => |a| {
            if let (Some(src), Some(dst)) = (view_builder(a.0[1], a.0[2]), view_builder(a.0[4], a.0[5])) {
                push(a.0[0], Cmd::CopyTextureToTexture { src, src_region: rd(a.0[3]), dst, dst_region: rd(a.0[6]), flags: a.0[7] as u32 });
            }
            0
        },
        "nvnCommandBufferCopyTextureToBuffer" => |a| {
            if let Some(src) = view_builder(a.0[1], a.0[2]) {
                push(a.0[0], Cmd::CopyTextureToBuffer { src, region: rd(a.0[3]), dst: a.0[4] });
            }
            0
        },
        "nvnCommandBufferClearTexture" => |a| {
            if let Some(dst) = view_builder(a.0[1], a.0[2]) {
                push(a.0[0], Cmd::ClearTexture { dst, region: rd(a.0[3]), color: rd(a.0[4]), mask: a.0[5] as u32 });
            }
            0
        },
        "nvnCommandBufferDispatchCompute" => |a| {
            push(a.0[0], Cmd::Dispatch([a.0[1] as u32, a.0[2] as u32, a.0[3] as u32]));
            0
        },
        "nvnCommandBufferSetRenderEnable" => |a| {
            push(a.0[0], Cmd::RenderEnable(a.0[1] & 0xFF != 0));
            0
        },
        "nvnCommandBufferSetRenderEnableConditional" => |a| {
            push(a.0[0], Cmd::RenderEnable(true));
            0
        },
        "nvnCommandBufferReportCounter" => |a| {
            push(a.0[0], Cmd::ReportCounter { counter: a.0[1] as u32, addr: a.0[2] });
            0
        },
        "nvnCommandBufferSetRasterizerDiscard" => |a| {
            push(a.0[0], Cmd::RasterizerDiscard(a.0[1] & 0xFF != 0));
            0
        },
        "nvnCommandBufferSetDepthRange" => |a| {
            push(a.0[0], Cmd::DepthRange(a.f32(0), a.f32(1)));
            0
        },
        "nvnCommandBufferSetPolygonOffsetClamp" => |a| {
            push(a.0[0], Cmd::PolygonOffset([a.f32(0), a.f32(1), a.f32(2)]));
            0
        },
        "nvnCommandBufferSetDepthClamp" => |a| {
            push(a.0[0], Cmd::DepthClamp(a.0[1] & 0xFF != 0));
            0
        },
        "nvnCommandBufferDrawArrays" => |a| draw(a, Draw { mode: a.0[1] as u32, index: None, first: a.0[2] as u32, count: a.0[3] as u32, base_vertex: 0, base_instance: 0, instances: 1 }),
        "nvnCommandBufferDrawArraysInstanced" => |a| {
            draw(a, Draw { mode: a.0[1] as u32, index: None, first: a.0[2] as u32, count: a.0[3] as u32, base_vertex: 0, base_instance: a.0[4] as u32, instances: a.0[5] as u32 })
        },
        "nvnCommandBufferDrawElements" => |a| {
            draw(a, Draw { mode: a.0[1] as u32, index: Some((a.0[2] as u32, a.0[4])), first: 0, count: a.0[3] as u32, base_vertex: 0, base_instance: 0, instances: 1 })
        },
        "nvnCommandBufferDrawElementsBaseVertex" => |a| {
            draw(a, Draw { mode: a.0[1] as u32, index: Some((a.0[2] as u32, a.0[4])), first: 0, count: a.0[3] as u32, base_vertex: a.0[5] as i32, base_instance: 0, instances: 1 })
        },
        "nvnCommandBufferDrawElementsInstanced" => |a| {
            draw(
                a,
                Draw {
                    mode: a.0[1] as u32,
                    index: Some((a.0[2] as u32, a.0[4])),
                    first: 0,
                    count: a.0[3] as u32,
                    base_vertex: a.0[5] as i32,
                    base_instance: a.0[6] as u32,
                    instances: a.0[7] as u32,
                },
            )
        },
        _ => return None,
    })
}

fn view_builder(texture: u64, view: u64) -> Option<Box<TexBuilder>> {
    let mut b = Box::new(super::objects::texture(texture)?.builder);
    super::objects::apply_view(&mut b, view, true);
    Some(b)
}

fn state_cmd(a: &Args, f: impl Fn(u64) -> Cmd) -> u64 {
    if a.0[1] != 0 {
        push(a.0[0], f(a.0[1]));
    }
    0
}

fn draw(a: &Args, d: Draw) -> u64 {
    if let Err(reason) = d.validate_indices() {
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| eprintln!("[nvn] skipping draw in command buffer {:#x}: {reason}; {d:?}", a.0[0]));
        return 0;
    }
    push(a.0[0], Cmd::Draw(d));
    0
}

pub fn submit(a: &Args) -> u64 {
    let handles = rd_vec::<u64>(a.0[2], a.0[1]);
    draw::submit(&handles);
    0
}

pub fn fence_sync(a: &Args) -> u64 {
    draw::fence_sync(a.0[1]);
    0
}

pub fn sync_wait(a: &Args) -> u64 {
    draw::sync_wait(a.0[0])
}

pub fn present(a: &Args) -> u64 {
    let tex = STATE.lock().unwrap().windows.get(&a.0[1]).and_then(|(ts, _)| ts.get(a.0[2] as usize).copied());
    if let Some(t) = tex {
        draw::present(t);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    fn record_draw(name: &str, mut args: [u64; 8]) -> std::sync::Arc<Vec<Cmd>> {
        static NEXT_CB: AtomicU64 = AtomicU64::new(0x1000);
        let cb = NEXT_CB.fetch_add(1, Relaxed);
        args[0] = cb;
        begin(cb);
        host_impl(name).unwrap()(&Args(args, [0; 8]));
        recording(end(cb)).unwrap()
    }

    #[test]
    fn indexed_draws_with_null_buffers_are_not_recorded() {
        for name in ["nvnCommandBufferDrawElements", "nvnCommandBufferDrawElementsBaseVertex", "nvnCommandBufferDrawElementsInstanced"] {
            let commands = record_draw(name, [0, 4, 1, 36, 0, 0, 0, 1]);
            assert!(commands.is_empty(), "{name} recorded a draw that would dereference null");
        }
    }

    #[test]
    fn overflowing_index_ranges_are_not_recorded() {
        let commands = record_draw("nvnCommandBufferDrawElements", [0, 4, 1, 2, u64::MAX - 1, 0, 0, 1]);
        assert!(commands.is_empty(), "index range overflow reached the renderer");
    }

    #[test]
    fn valid_indexed_draw_preserves_arguments() {
        let indices = [0u16, 1, 2];
        let addr = indices.as_ptr() as u64;
        let commands = record_draw("nvnCommandBufferDrawElementsInstanced", [0, 4, 1, 3, addr, 7, 2, 4]);
        let [Cmd::Draw(d)] = commands.as_slice() else { panic!("expected one indexed draw") };
        assert_eq!(d.index, Some((1, addr)));
        assert_eq!((d.mode, d.count, d.base_vertex, d.base_instance, d.instances), (4, 3, 7, 2, 4));
    }

    #[test]
    fn non_indexed_draw_does_not_require_an_index_buffer() {
        let commands = record_draw("nvnCommandBufferDrawArrays", [0, 4, 2, 36, 0, 0, 0, 0]);
        let [Cmd::Draw(d)] = commands.as_slice() else { panic!("expected one non-indexed draw") };
        assert_eq!(d.index, None);
        assert_eq!((d.first, d.count), (2, 36));
    }
}
