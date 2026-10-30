// Mush8 - decode.rs
// Constantin Clerc (c22dev) - 0.1
const EXPECTED: [u64; 45] = [
    0x8af1e558736948be, 0x9c6099d6d1a4d916, 0xa9d9d64ed76a7736, 0xbfaffe6d31208fe7, 0x4013a9ea695de632, 0xda624be19aba71e8, 0xf3e1f8ca4ad2f422, 0x613c0e5d8527c02c, 0x1d775c82309fe2c4,
    0x63f27d8a7226b247, 0xb56443a9946d1c23, 0x337b860dec817011, 0x4b715a61ee8addfe, 0x376f8376cf00590e, 0x4e31b42fae166ff2, 0xcab87edf41c9e7ed, 0x5990d8fdf709546b, 0x88d741b8dca7d79d,
    0x15bb2d674f1b3f77, 0xff19fdf7f0e426a5, 0x6561cd62c623d288, 0xc4663749bcc65041, 0x7c4f39411d7e5f0b, 0xd7b391c27035cd13, 0x242f390794835ea7, 0xd17d31d5074d5574, 0xf762a02ac57d55d2,
    0x4e8ec98238501268, 0x0d5209d10ebc2219, 0xe941aca12fd6f6e4, 0x489cea69b74030c7, 0xaf64f90337bdeeca, 0x8a17f1e774f417a3, 0x60b80706f33e6dd0, 0x23b807d05b425905, 0xed582ec87abef771,
    0xb24e132a651776f6, 0xc4226c782ebb0eff, 0xb009e01a35efaebe, 0xdfc445220e3bfa8b, 0x994b7a7be0d553ce, 0xe6c875f090effe41, 0xfa79d4f562fe8d1c, 0xa24399dcd088c3db, 0x9f25453215fb529f,
];

fn ivf_frames(d: &[u8]) -> Vec<&[u8]> {
    let mut p = u16::from_le_bytes([d[6], d[7]]) as usize;
    let mut out = Vec::new();
    while p + 12 <= d.len() {
        let n = u32::from_le_bytes(d[p..p + 4].try_into().unwrap()) as usize;
        out.push(&d[p + 12..p + 12 + n]);
        p += 12 + n;
    }
    out
}

#[test]
fn decodes_every_frame_like_ffmpeg() {
    let mut dec = vpx::Vp8::new().unwrap();
    let mut got = Vec::new();
    for f in ivf_frames(include_bytes!("testsrc.ivf")) {
        let img = dec.decode(f).unwrap().unwrap();
        assert_eq!((img.width, img.height), (98, 66));
        let mut h = 0xcbf29ce484222325u64;
        for (i, plane) in img.planes.iter().enumerate() {
            let (w, rows) = if i == 0 { (98, 66) } else { (49, 33) };
            for y in 0..rows {
                for &b in &plane[y * img.strides[i]..y * img.strides[i] + w] {
                    h = (h ^ b as u64).wrapping_mul(0x100000001b3);
                }
            }
        }
        got.push(h);
    }
    assert_eq!(got, EXPECTED);
}
