// Mush8 - lib.rs
// Constantin Clerc (c22dev) - 0.1
use std::ffi::{c_char, c_int, c_long, c_uint, c_void};

#[repr(C)]
struct Ctx {
    name: *const c_char,
    iface: *const c_void,
    err: c_int,
    err_detail: *const c_char,
    init_flags: c_long,
    config: *const c_void,
    private: *mut c_void,
}

#[repr(C)]
struct RawImage {
    fmt: c_int,
    cs: c_int,
    range: c_int,
    w: c_uint,
    h: c_uint,
    bit_depth: c_uint,
    d_w: c_uint,
    d_h: c_uint,
    r_w: c_uint,
    r_h: c_uint,
    x_chroma_shift: c_uint,
    y_chroma_shift: c_uint,
    planes: [*const u8; 4],
    stride: [c_int; 4],
    bps: c_int,
    user_priv: *mut c_void,
    img_data: *mut u8,
    img_data_owner: c_int,
    self_allocd: c_int,
    fb_priv: *mut c_void,
}

const IMAGE_ABI: c_int = 5;
const CODEC_ABI: c_int = 4 + IMAGE_ABI;
const DECODER_ABI: c_int = 3 + CODEC_ABI;
const FMT_I420: c_int = 0x102;

unsafe extern "C" {
    fn vpx_codec_vp8_dx() -> *const c_void;
    fn vpx_codec_dec_init_ver(ctx: *mut Ctx, iface: *const c_void, cfg: *const c_void, flags: c_long, ver: c_int) -> c_int;
    fn vpx_codec_decode(ctx: *mut Ctx, data: *const u8, size: c_uint, user_priv: *mut c_void, deadline: c_long) -> c_int;
    fn vpx_codec_get_frame(ctx: *mut Ctx, iter: *mut *const c_void) -> *const RawImage;
    fn vpx_codec_destroy(ctx: *mut Ctx) -> c_int;
}

pub struct Image<'a> {
    pub width: usize,
    pub height: usize,
    pub planes: [&'a [u8]; 3],
    pub strides: [usize; 3],
}

pub struct Vp8 {
    ctx: Box<Ctx>,
}

unsafe impl Send for Vp8 {}

impl Vp8 {
    pub fn new() -> Option<Self> {
        let mut ctx = Box::new(Ctx {
            name: std::ptr::null(),
            iface: std::ptr::null(),
            err: 0,
            err_detail: std::ptr::null(),
            init_flags: 0,
            config: std::ptr::null(),
            private: std::ptr::null_mut(),
        });
        let err = unsafe { vpx_codec_dec_init_ver(&mut *ctx, vpx_codec_vp8_dx(), std::ptr::null(), 0, DECODER_ABI) };
        (err == 0).then_some(Self { ctx })
    }

    pub fn decode(&mut self, data: &[u8]) -> Result<Option<Image<'_>>, i32> {
        let err = unsafe { vpx_codec_decode(&mut *self.ctx, data.as_ptr(), data.len() as c_uint, std::ptr::null_mut(), 0) };
        if err != 0 {
            return Err(err);
        }
        let mut iter = std::ptr::null();
        let mut last = std::ptr::null();
        loop {
            let img = unsafe { vpx_codec_get_frame(&mut *self.ctx, &mut iter) };
            if img.is_null() {
                break;
            }
            last = img;
        }
        let Some(img) = (unsafe { last.as_ref() }) else { return Ok(None) };
        if img.fmt != FMT_I420 {
            return Err(-1);
        }
        let (width, height) = (img.d_w as usize, img.d_h as usize);
        let dims = [(width, height), (width.div_ceil(2), height.div_ceil(2)), (width.div_ceil(2), height.div_ceil(2))];
        let strides = [0, 1, 2].map(|i| img.stride[i] as usize);
        let planes = [0, 1, 2].map(|i| {
            let (w, h) = dims[i];
            let len = if h == 0 { 0 } else { (h - 1) * strides[i] + w };
            unsafe { std::slice::from_raw_parts(img.planes[i], len) }
        });
        Ok(Some(Image { width, height, planes, strides }))
    }
}

impl Drop for Vp8 {
    fn drop(&mut self) {
        unsafe { vpx_codec_destroy(&mut *self.ctx) };
    }
}
