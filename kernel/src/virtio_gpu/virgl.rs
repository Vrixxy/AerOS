//! virgl 3D on top of the virtio-gpu control queue: a small, fixed pipeline
//! that blurs and colour-grades a rectangle of pixels on the host GPU.
//!
//! The desktop's frosted glass spends most of its time in a separable box
//! blur plus a per-pixel colour transform. `frost_pass` uploads the backdrop
//! rectangle to a texture, draws it twice (horizontal taps into a temporary
//! texture, then vertical taps with saturation/brightness/tint into the
//! result texture) and reads the result back. The shaders are TGSI text, the
//! only shader language virglrenderer accepts, generated here with the tap
//! count baked in and the tap offsets and weights supplied as constants.
//!
//! Everything about the protocol (object types, command numbers, payload
//! layouts) follows virglrenderer's `virgl_protocol.h`.

use super::{
    CtrlHeader, GpuState, RESP_OFFSET, RESP_OK_NODATA, attach_backing, exchange_raw, response_type,
};
use crate::memory::FrameAllocator;
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};

/// Where the GPU glass time goes, summed until `take_stats`.
static STAT_CALLS: AtomicU64 = AtomicU64::new(0);
static STAT_PIXELS: AtomicU64 = AtomicU64::new(0);
static STAT_UPLOAD_NS: AtomicU64 = AtomicU64::new(0);
static STAT_TO_NS: AtomicU64 = AtomicU64::new(0);
static STAT_DRAW_NS: AtomicU64 = AtomicU64::new(0);
static STAT_FROM_NS: AtomicU64 = AtomicU64::new(0);
static STAT_DOWNLOAD_NS: AtomicU64 = AtomicU64::new(0);

fn now() -> u64 {
    crate::time::monotonic_nanoseconds()
}

fn add(counter: &AtomicU64, since: u64) -> u64 {
    let current = now();
    counter.fetch_add(current.saturating_sub(since), Ordering::Relaxed);
    current
}

/// (calls, pixels, upload, to-host, draw, from-host, download) in
/// microseconds, reset on read.
pub fn take_stats() -> [u64; 7] {
    [
        STAT_CALLS.swap(0, Ordering::Relaxed),
        STAT_PIXELS.swap(0, Ordering::Relaxed),
        STAT_UPLOAD_NS.swap(0, Ordering::Relaxed) / 1000,
        STAT_TO_NS.swap(0, Ordering::Relaxed) / 1000,
        STAT_DRAW_NS.swap(0, Ordering::Relaxed) / 1000,
        STAT_FROM_NS.swap(0, Ordering::Relaxed) / 1000,
        STAT_DOWNLOAD_NS.swap(0, Ordering::Relaxed) / 1000,
    ]
}

const CMD_GET_CAPSET_INFO: u32 = 0x0108;
const CMD_CTX_CREATE: u32 = 0x0200;
const CMD_CTX_ATTACH_RESOURCE: u32 = 0x0202;
const CMD_RESOURCE_CREATE_3D: u32 = 0x0204;
const CMD_TRANSFER_TO_HOST_3D: u32 = 0x0205;
const CMD_TRANSFER_FROM_HOST_3D: u32 = 0x0206;
const CMD_SUBMIT_3D: u32 = 0x0207;
const RESP_OK_CAPSET_INFO: u32 = 0x1102;

const CTX_ID: u32 = 1;
const RES_SRC: u32 = 0x40;
const RES_TMP: u32 = 0x41;
const RES_DST: u32 = 0x42;
const RES_VB: u32 = 0x43;

// virgl object handles (guest-chosen, per object type).
const H_BLEND: u32 = 1;
const H_DSA: u32 = 2;
const H_RASTERIZER: u32 = 3;
const H_VERTEX_ELEMENTS: u32 = 4;
const H_SAMPLER: u32 = 5;
const H_VS: u32 = 6;
const H_FS_HORIZONTAL: u32 = 7;
const H_FS_VERTICAL: u32 = 8;
const H_SURFACE_TMP: u32 = 9;
const H_SURFACE_DST: u32 = 10;
const H_VIEW_SRC: u32 = 11;
const H_VIEW_TMP: u32 = 12;

// virgl_context_cmd
const CCMD_CREATE_OBJECT: u32 = 1;
const CCMD_BIND_OBJECT: u32 = 2;
const CCMD_SET_VIEWPORT_STATE: u32 = 4;
const CCMD_SET_FRAMEBUFFER_STATE: u32 = 5;
const CCMD_SET_VERTEX_BUFFERS: u32 = 6;
const CCMD_DRAW_VBO: u32 = 8;
const CCMD_SET_SAMPLER_VIEWS: u32 = 10;
const CCMD_SET_CONSTANT_BUFFER: u32 = 12;
const CCMD_BIND_SAMPLER_STATES: u32 = 18;
const CCMD_BIND_SHADER: u32 = 31;

// virgl_object_type
const OBJ_BLEND: u32 = 1;
const OBJ_RASTERIZER: u32 = 2;
const OBJ_DSA: u32 = 3;
const OBJ_SHADER: u32 = 4;
const OBJ_VERTEX_ELEMENTS: u32 = 5;
const OBJ_SAMPLER_VIEW: u32 = 6;
const OBJ_SAMPLER_STATE: u32 = 7;
const OBJ_SURFACE: u32 = 8;

const SHADER_VERTEX: u32 = 0;
const SHADER_FRAGMENT: u32 = 1;

const FORMAT_B8G8R8A8_UNORM: u32 = 1;
const FORMAT_R8_UNORM: u32 = 64;
const FORMAT_R32G32_FLOAT: u32 = 29;
const FORMAT_R32G32B32A32_FLOAT: u32 = 31;

const TARGET_BUFFER: u32 = 0;
const TARGET_TEXTURE_2D: u32 = 2;

const BIND_RENDER_TARGET: u32 = 1 << 1;
const BIND_SAMPLER_VIEW: u32 = 1 << 3;
const BIND_VERTEX_BUFFER: u32 = 1 << 4;

const PRIM_TRIANGLE_STRIP: u32 = 5;

/// Taps per pass: a box of up to this radius on each side of the centre.
pub const MAX_RADIUS: usize = 32;
const TAPS: usize = MAX_RADIUS * 2 + 1;
/// Constant slots: the taps, the clamp rectangle, then the colour transform.
const CONST_CLAMP: usize = TAPS;
const CONST_GRADE: usize = TAPS + 1;
const CONST_TINT: usize = TAPS + 2;

const COMMAND_BYTES: usize = 64 * 1024;
const SUBMIT_HEADER_BYTES: usize = 32;
const VERTEX_STRIDE: u32 = 24;

/// Colour transform applied after the vertical pass.
#[derive(Clone, Copy)]
pub struct Grade {
    pub saturation: f32,
    pub brightness: f32,
    pub tint: [f32; 4],
}

pub struct State {
    pub width: u32,
    pub height: u32,
    command: u64,
    src_backing: u64,
    dst_backing: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CtxCreate {
    hdr: CtrlHeader,
    name_length: u32,
    context_init: u32,
    name: [u8; 64],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ResourceCreate3d {
    hdr: CtrlHeader,
    resource_id: u32,
    target: u32,
    format: u32,
    bind: u32,
    width: u32,
    height: u32,
    depth: u32,
    array_size: u32,
    last_level: u32,
    nr_samples: u32,
    flags: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CtxResource {
    hdr: CtrlHeader,
    resource_id: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Box3d {
    x: u32,
    y: u32,
    z: u32,
    width: u32,
    height: u32,
    depth: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TransferHost3d {
    hdr: CtrlHeader,
    region: Box3d,
    offset: u64,
    resource_id: u32,
    level: u32,
    stride: u32,
    layer_stride: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SubmitHeader {
    hdr: CtrlHeader,
    size: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CapsetInfoRequest {
    hdr: CtrlHeader,
    index: u32,
    padding: u32,
}

fn header(cmd_type: u32) -> CtrlHeader {
    CtrlHeader {
        cmd_type,
        flags: 0,
        fence_id: 0,
        ctx_id: CTX_ID,
        ring_idx: 0,
        padding: [0; 3],
    }
}

fn simple<T>(gpu: &mut GpuState, command: T) -> bool {
    let address = gpu.control;
    unsafe { core::ptr::write_volatile(address as *mut T, command) };
    exchange_raw(gpu, address, core::mem::size_of::<T>() as u32, 24)
        && response_type(gpu.control) == RESP_OK_NODATA
}

/// A command stream being written into the DMA command buffer.
struct Stream {
    base: u64,
    words: usize,
    capacity: usize,
    overflow: bool,
}

impl Stream {
    fn new(command: u64) -> Self {
        Self {
            base: command + SUBMIT_HEADER_BYTES as u64,
            words: 0,
            capacity: (COMMAND_BYTES - SUBMIT_HEADER_BYTES) / 4,
            overflow: false,
        }
    }

    fn push(&mut self, value: u32) {
        if self.words >= self.capacity {
            self.overflow = true;
            return;
        }
        unsafe {
            core::ptr::write_volatile((self.base as usize + self.words * 4) as *mut u32, value)
        };
        self.words += 1;
    }

    fn float(&mut self, value: f32) {
        self.push(value.to_bits());
    }

    fn command(&mut self, cmd: u32, object: u32, length: usize) {
        self.push(cmd | (object << 8) | ((length as u32) << 16));
    }
}

/// A `core::fmt::Write` sink over raw bytes (the shader text lives inside
/// the command buffer, so no intermediate copy is needed).
struct TextSink {
    base: usize,
    length: usize,
    capacity: usize,
}

impl Write for TextSink {
    fn write_str(&mut self, value: &str) -> core::fmt::Result {
        let bytes = value.as_bytes();
        if self.length + bytes.len() > self.capacity {
            return Err(core::fmt::Error);
        }
        for (index, byte) in bytes.iter().enumerate() {
            unsafe {
                core::ptr::write_volatile((self.base + self.length + index) as *mut u8, *byte)
            };
        }
        self.length += bytes.len();
        Ok(())
    }
}

fn write_vertex_shader(text: &mut TextSink) -> core::fmt::Result {
    text.write_str(
        "VERT\n\
         DCL IN[0]\n\
         DCL IN[1]\n\
         DCL OUT[0], POSITION\n\
         DCL OUT[1], GENERIC[0]\n\
         DCL CONST[0]\n\
         MOV OUT[0], IN[0]\n\
         MUL OUT[1], IN[1], CONST[0]\n\
         END\n",
    )
}

fn write_fragment_shader(text: &mut TextSink, grade: bool) -> core::fmt::Result {
    write!(
        text,
        "FRAG\n\
         PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\n\
         DCL IN[0], GENERIC[0], PERSPECTIVE\n\
         DCL OUT[0], COLOR\n\
         DCL SAMP[0]\n\
         DCL CONST[0..{last}]\n\
         DCL TEMP[0..4]\n\
         IMM[0] FLT32 {{ 0.0, 0.0, 0.0, 1.0 }}\n\
         IMM[1] FLT32 {{ 0.2109, 0.7148, 0.0742, 0.0 }}\n\
         MOV TEMP[0], IMM[0].xxxx\n",
        last = CONST_TINT
    )?;
    for tap in 0..TAPS {
        write!(
            text,
            "ADD TEMP[1].xy, IN[0].xyxy, CONST[{tap}].xyxy\n\
             MAX TEMP[1].xy, TEMP[1].xyxy, CONST[{clamp}].xyxy\n\
             MIN TEMP[1].xy, TEMP[1].xyxy, CONST[{clamp}].zwzw\n\
             TEX TEMP[2], TEMP[1], SAMP[0], 2D\n\
             MAD TEMP[0], TEMP[2], CONST[{tap}].zzzz, TEMP[0]\n",
            clamp = CONST_CLAMP
        )?;
    }
    if grade {
        write!(
            text,
            "DP3 TEMP[3].x, TEMP[0], IMM[1]\n\
             SUB TEMP[4], TEMP[0], TEMP[3].xxxx\n\
             MAD TEMP[0], TEMP[4], CONST[{grade}].xxxx, TEMP[3].xxxx\n\
             MUL TEMP[0], TEMP[0], CONST[{grade}].yyyy\n\
             LRP TEMP[0], CONST[{grade}].zzzz, CONST[{tint}], TEMP[0]\n",
            grade = CONST_GRADE,
            tint = CONST_TINT
        )?;
    }
    text.write_str("MOV TEMP[0].w, IMM[0].wwww\nMOV OUT[0], TEMP[0]\nEND\n")
}

/// Appends a CREATE_OBJECT(SHADER) whose text is produced by `build`.
fn push_shader(
    stream: &mut Stream,
    handle: u32,
    shader_type: u32,
    build: &dyn Fn(&mut TextSink) -> core::fmt::Result,
) {
    let start = stream.words;
    let header_words = 6; // command header + handle,type,offlen,tokens,so
    if start + header_words + 4 >= stream.capacity {
        stream.overflow = true;
        return;
    }
    let text_base = stream.base as usize + (start + header_words) * 4;
    let room = (stream.capacity - start - header_words) * 4;
    let mut sink = TextSink {
        base: text_base,
        length: 0,
        capacity: room.saturating_sub(4),
    };
    if build(&mut sink).is_err() {
        stream.overflow = true;
        return;
    }
    let length = sink.length;
    // NUL terminate and pad to a whole number of words with zero bytes.
    let padded = (length + 1).next_multiple_of(4);
    for index in length..padded {
        unsafe { core::ptr::write_volatile((text_base + index) as *mut u8, 0) };
    }
    let text_words = padded / 4;
    stream.words = start;
    stream.command(CCMD_CREATE_OBJECT, OBJ_SHADER, 5 + text_words);
    stream.push(handle);
    stream.push(shader_type);
    stream.push((length + 1) as u32);
    stream.push(8192); // an upper bound on the token count
    stream.push(0); // no stream output
    stream.words += text_words;
}

fn submit(gpu: &mut GpuState, state: &State, stream: &Stream) -> bool {
    if stream.overflow {
        return false;
    }
    let command = SubmitHeader {
        hdr: header(CMD_SUBMIT_3D),
        size: (stream.words * 4) as u32,
        padding: 0,
    };
    unsafe { core::ptr::write_volatile(state.command as *mut SubmitHeader, command) };
    exchange_raw(
        gpu,
        state.command,
        (SUBMIT_HEADER_BYTES + stream.words * 4) as u32,
        24,
    ) && response_type(gpu.control) == RESP_OK_NODATA
}

fn transfer(
    gpu: &mut GpuState,
    cmd_type: u32,
    resource_id: u32,
    width: u32,
    height: u32,
    stride: u32,
) -> bool {
    simple(
        gpu,
        TransferHost3d {
            hdr: header(cmd_type),
            region: Box3d {
                x: 0,
                y: 0,
                z: 0,
                width,
                height,
                depth: 1,
            },
            offset: 0,
            resource_id,
            level: 0,
            stride,
            layer_stride: 0,
        },
    )
}

fn create_resource(
    gpu: &mut GpuState,
    resource_id: u32,
    target: u32,
    format: u32,
    bind: u32,
    width: u32,
    height: u32,
) -> bool {
    simple(
        gpu,
        ResourceCreate3d {
            hdr: header(CMD_RESOURCE_CREATE_3D),
            resource_id,
            target,
            format,
            bind,
            width,
            height,
            depth: 1,
            array_size: 1,
            last_level: 0,
            nr_samples: 0,
            flags: 0,
            padding: 0,
        },
    )
}

fn attach_to_context(gpu: &mut GpuState, resource_id: u32) -> bool {
    simple(
        gpu,
        CtxResource {
            hdr: header(CMD_CTX_ATTACH_RESOURCE),
            resource_id,
            padding: 0,
        },
    )
}

/// Capset count the device reports in its config space, 0 when absent.
pub fn capset_count(gpu: &GpuState) -> u32 {
    if !gpu.modern.has_device_config() {
        return 0;
    }
    gpu.modern.device_config_read32(12) as u32
}

fn capset_info(gpu: &mut GpuState) -> (u32, u32, u32) {
    let command = CapsetInfoRequest {
        hdr: header(CMD_GET_CAPSET_INFO),
        index: 0,
        padding: 0,
    };
    let control = gpu.control;
    unsafe { core::ptr::write_volatile(control as *mut CapsetInfoRequest, command) };
    if !exchange_raw(gpu, control, 32, 40) || response_type(gpu.control) != RESP_OK_CAPSET_INFO {
        return (0, 0, 0);
    }
    let base = gpu.control + RESP_OFFSET + 24;
    unsafe {
        (
            core::ptr::read_volatile(base as *const u32),
            core::ptr::read_volatile((base + 4) as *const u32),
            core::ptr::read_volatile((base + 8) as *const u32),
        )
    }
}

fn create_state_objects(stream: &mut Stream) {
    // Blend: no blending, all colour channels written.
    stream.command(CCMD_CREATE_OBJECT, OBJ_BLEND, 11);
    stream.push(H_BLEND);
    stream.push(0);
    stream.push(0);
    stream.push(0xf << 27);
    for _ in 1..8 {
        stream.push(0);
    }
    // Depth/stencil/alpha: all off.
    stream.command(CCMD_CREATE_OBJECT, OBJ_DSA, 5);
    stream.push(H_DSA);
    for _ in 0..4 {
        stream.push(0);
    }
    // Rasterizer: fill, no culling, no clipping surprises.
    stream.command(CCMD_CREATE_OBJECT, OBJ_RASTERIZER, 9);
    stream.push(H_RASTERIZER);
    stream.push((1 << 1) | (1 << 29));
    stream.float(1.0);
    stream.push(0);
    stream.push(0);
    stream.float(1.0);
    stream.push(0);
    stream.push(0);
    stream.push(0);
    // Vertex elements: position (xyzw) and texture coordinate (uv).
    stream.command(CCMD_CREATE_OBJECT, OBJ_VERTEX_ELEMENTS, 9);
    stream.push(H_VERTEX_ELEMENTS);
    for (offset, format) in [(0u32, FORMAT_R32G32B32A32_FLOAT), (16, FORMAT_R32G32_FLOAT)] {
        stream.push(offset);
        stream.push(0);
        stream.push(0);
        stream.push(format);
    }
    // Sampler: nearest, clamp to edge, no mip filter.
    stream.command(CCMD_CREATE_OBJECT, OBJ_SAMPLER_STATE, 9);
    stream.push(H_SAMPLER);
    stream.push(2 | (2 << 3) | (2 << 6) | (2 << 11));
    for _ in 0..7 {
        stream.push(0);
    }
}

fn create_views(stream: &mut Stream) {
    for (handle, resource) in [(H_SURFACE_TMP, RES_TMP), (H_SURFACE_DST, RES_DST)] {
        stream.command(CCMD_CREATE_OBJECT, OBJ_SURFACE, 5);
        stream.push(handle);
        stream.push(resource);
        stream.push(FORMAT_B8G8R8A8_UNORM);
        stream.push(0);
        stream.push(0);
    }
    for (handle, resource) in [(H_VIEW_SRC, RES_SRC), (H_VIEW_TMP, RES_TMP)] {
        stream.command(CCMD_CREATE_OBJECT, OBJ_SAMPLER_VIEW, 6);
        stream.push(handle);
        stream.push(resource);
        stream.push(FORMAT_B8G8R8A8_UNORM);
        stream.push(0);
        stream.push(0);
        stream.push(1 << 3 | 2 << 6 | 3 << 9);
    }
}

/// Everything per context: creates the context, the four resources, uploads
/// the quad, and defines the fixed pipeline objects. None when the device
/// has no usable 3D support.
pub fn init(
    gpu: &mut GpuState,
    width: u32,
    height: u32,
    frames: &mut FrameAllocator,
) -> Option<State> {
    if capset_count(gpu) == 0 {
        return None;
    }
    let (_, max_version, max_size) = capset_info(gpu);
    let mut name = [0u8; 64];
    name[..5].copy_from_slice(b"aeros");
    if !simple(
        gpu,
        CtxCreate {
            hdr: header(CMD_CTX_CREATE),
            name_length: 5,
            context_init: 0,
            name,
        },
    ) {
        return None;
    }
    let pixel_bytes = width as u64 * height as u64 * 4;
    let pages = pixel_bytes.div_ceil(4096);
    let src_backing = frames.allocate_dma(pages, 1)?.address();
    let dst_backing = frames.allocate_dma(pages, 1)?.address();
    let command = frames
        .allocate_dma((COMMAND_BYTES / 4096) as u64, 1)?
        .address();
    let vertex = frames.allocate_dma(1, 1)?.address();
    unsafe {
        core::ptr::write_bytes(src_backing as usize as *mut u8, 0, (pages * 4096) as usize);
        core::ptr::write_bytes(dst_backing as usize as *mut u8, 0, (pages * 4096) as usize);
        core::ptr::write_bytes(command as usize as *mut u8, 0, COMMAND_BYTES);
        core::ptr::write_bytes(vertex as usize as *mut u8, 0, 4096);
    }
    let quad: [f32; 24] = [
        -1.0, -1.0, 0.0, 1.0, 0.0, 0.0, //
        1.0, -1.0, 0.0, 1.0, 1.0, 0.0, //
        -1.0, 1.0, 0.0, 1.0, 0.0, 1.0, //
        1.0, 1.0, 0.0, 1.0, 1.0, 1.0,
    ];
    for (index, value) in quad.iter().enumerate() {
        unsafe {
            core::ptr::write_volatile((vertex as usize + index * 4) as *mut u32, value.to_bits())
        };
    }
    let flags_texture = BIND_SAMPLER_VIEW | BIND_RENDER_TARGET;
    if !create_resource(
        gpu,
        RES_SRC,
        TARGET_TEXTURE_2D,
        FORMAT_B8G8R8A8_UNORM,
        flags_texture,
        width,
        height,
    ) || !create_resource(
        gpu,
        RES_TMP,
        TARGET_TEXTURE_2D,
        FORMAT_B8G8R8A8_UNORM,
        flags_texture,
        width,
        height,
    ) || !create_resource(
        gpu,
        RES_DST,
        TARGET_TEXTURE_2D,
        FORMAT_B8G8R8A8_UNORM,
        flags_texture,
        width,
        height,
    ) || !create_resource(
        gpu,
        RES_VB,
        TARGET_BUFFER,
        FORMAT_R8_UNORM,
        BIND_VERTEX_BUFFER,
        96,
        1,
    ) {
        return None;
    }
    if !attach_backing(gpu, RES_SRC, src_backing, pixel_bytes as u32)
        || !attach_backing(gpu, RES_DST, dst_backing, pixel_bytes as u32)
        || !attach_backing(gpu, RES_VB, vertex, 4096)
    {
        return None;
    }
    for resource in [RES_SRC, RES_TMP, RES_DST, RES_VB] {
        if !attach_to_context(gpu, resource) {
            return None;
        }
    }
    let state = State {
        width,
        height,
        command,
        src_backing,
        dst_backing,
    };
    // The quad: a 96-byte buffer upload.
    if !simple(
        gpu,
        TransferHost3d {
            hdr: header(CMD_TRANSFER_TO_HOST_3D),
            region: Box3d {
                x: 0,
                y: 0,
                z: 0,
                width: 96,
                height: 1,
                depth: 1,
            },
            offset: 0,
            resource_id: RES_VB,
            level: 0,
            stride: 0,
            layer_stride: 0,
        },
    ) {
        return None;
    }
    let mut stream = Stream::new(command);
    create_state_objects(&mut stream);
    create_views(&mut stream);
    if !submit(gpu, &state, &stream) {
        return None;
    }
    let mut stream = Stream::new(command);
    push_shader(&mut stream, H_VS, SHADER_VERTEX, &|text| {
        write_vertex_shader(text)
    });
    if !submit(gpu, &state, &stream) {
        return None;
    }
    let mut stream = Stream::new(command);
    push_shader(&mut stream, H_FS_HORIZONTAL, SHADER_FRAGMENT, &|text| {
        write_fragment_shader(text, false)
    });
    if !submit(gpu, &state, &stream) {
        return None;
    }
    let mut stream = Stream::new(command);
    push_shader(&mut stream, H_FS_VERTICAL, SHADER_FRAGMENT, &|text| {
        write_fragment_shader(text, true)
    });
    if !submit(gpu, &state, &stream) {
        return None;
    }
    crate::serial::format(format_args!(
        "AEROS_VIRGL context=true capset_version={max_version} capset_size={max_size} width={width} height={height}\n"
    ));
    Some(state)
}

#[allow(clippy::too_many_arguments)]
fn draw_pass(
    stream: &mut Stream,
    surface: u32,
    view: u32,
    fragment: u32,
    width: u32,
    height: u32,
    taps: &[[f32; 4]; TAPS],
    clamp: [f32; 4],
    grade: Option<Grade>,
) {
    stream.command(CCMD_SET_FRAMEBUFFER_STATE, 0, 3);
    stream.push(1);
    stream.push(0);
    stream.push(surface);

    stream.command(CCMD_SET_VIEWPORT_STATE, 0, 7);
    stream.push(0);
    stream.float(width as f32 / 2.0);
    stream.float(height as f32 / 2.0);
    stream.float(0.5);
    stream.float(width as f32 / 2.0);
    stream.float(height as f32 / 2.0);
    stream.float(0.5);

    for (object, handle) in [
        (OBJ_BLEND, H_BLEND),
        (OBJ_DSA, H_DSA),
        (OBJ_RASTERIZER, H_RASTERIZER),
        (OBJ_VERTEX_ELEMENTS, H_VERTEX_ELEMENTS),
    ] {
        stream.command(CCMD_BIND_OBJECT, object, 1);
        stream.push(handle);
    }

    stream.command(CCMD_BIND_SHADER, 0, 2);
    stream.push(H_VS);
    stream.push(SHADER_VERTEX);
    stream.command(CCMD_BIND_SHADER, 0, 2);
    stream.push(fragment);
    stream.push(SHADER_FRAGMENT);

    stream.command(CCMD_SET_SAMPLER_VIEWS, 0, 3);
    stream.push(SHADER_FRAGMENT);
    stream.push(0);
    stream.push(view);
    stream.command(CCMD_BIND_SAMPLER_STATES, 0, 3);
    stream.push(SHADER_FRAGMENT);
    stream.push(0);
    stream.push(H_SAMPLER);

    let constants = TAPS + 3;
    stream.command(CCMD_SET_CONSTANT_BUFFER, 0, 2 + constants * 4);
    stream.push(SHADER_FRAGMENT);
    stream.push(0);
    for tap in taps {
        for value in tap {
            stream.float(*value);
        }
    }
    for value in clamp {
        stream.float(value);
    }
    let grade = grade.unwrap_or(Grade {
        saturation: 1.0,
        brightness: 1.0,
        tint: [0.0; 4],
    });
    stream.float(grade.saturation);
    stream.float(grade.brightness);
    stream.float(grade.tint[3]);
    stream.float(0.0);
    stream.float(grade.tint[0]);
    stream.float(grade.tint[1]);
    stream.float(grade.tint[2]);
    stream.float(1.0);

    stream.command(CCMD_SET_VERTEX_BUFFERS, 0, 3);
    stream.push(VERTEX_STRIDE);
    stream.push(0);
    stream.push(RES_VB);

    stream.command(CCMD_DRAW_VBO, 0, 12);
    stream.push(0);
    stream.push(4);
    stream.push(PRIM_TRIANGLE_STRIP);
    stream.push(0);
    stream.push(1);
    stream.push(0);
    stream.push(0);
    stream.push(0);
    stream.push(0);
    stream.push(0);
    stream.push(3);
    stream.push(0);
}

/// Blurs and grades a `width` x `height` rectangle. `upload` fills the source
/// backing (BGRA bytes, rows `stride` bytes apart); `download` receives the
/// result the same way. False when anything failed, in which case nothing
/// was downloaded.
#[allow(clippy::too_many_arguments)]
pub fn frost_pass(
    gpu: &mut GpuState,
    state: &State,
    width: u32,
    height: u32,
    radius: u32,
    grade: Grade,
    upload: &mut dyn FnMut(&mut [u8], usize),
    download: &mut dyn FnMut(&[u8], usize),
) -> bool {
    if width == 0 || height == 0 || width > state.width || height > state.height {
        return false;
    }
    let radius = (radius as usize).min(MAX_RADIUS);
    let stride = state.width as usize * 4;
    let total = stride * state.height as usize;
    let mark = now();
    let source =
        unsafe { core::slice::from_raw_parts_mut(state.src_backing as usize as *mut u8, total) };
    upload(source, stride);
    let mark = add(&STAT_UPLOAD_NS, mark);
    if !transfer(
        gpu,
        CMD_TRANSFER_TO_HOST_3D,
        RES_SRC,
        width,
        height,
        stride as u32,
    ) {
        return false;
    }
    let mark = add(&STAT_TO_NS, mark);
    let texel_u = 1.0 / state.width as f32;
    let texel_v = 1.0 / state.height as f32;
    let weight = 1.0 / (2 * radius + 1) as f32;
    let clamp = [
        0.5 * texel_u,
        0.5 * texel_v,
        (width as f32 - 0.5) * texel_u,
        (height as f32 - 0.5) * texel_v,
    ];
    let mut horizontal = [[0.0f32; 4]; TAPS];
    let mut vertical = [[0.0f32; 4]; TAPS];
    for (index, (h, v)) in horizontal.iter_mut().zip(vertical.iter_mut()).enumerate() {
        let offset = index as i32 - MAX_RADIUS as i32;
        let active = offset.unsigned_abs() as usize <= radius;
        let w = if active { weight } else { 0.0 };
        let step = if active { offset as f32 } else { 0.0 };
        *h = [step * texel_u, 0.0, w, 0.0];
        *v = [0.0, step * texel_v, w, 0.0];
    }
    let mut stream = Stream::new(state.command);
    // Vertex shader constant: the quad's uv range covers only the rectangle.
    stream.command(CCMD_SET_CONSTANT_BUFFER, 0, 6);
    stream.push(SHADER_VERTEX);
    stream.push(0);
    stream.float(width as f32 * texel_u);
    stream.float(height as f32 * texel_v);
    stream.float(1.0);
    stream.float(1.0);
    draw_pass(
        &mut stream,
        H_SURFACE_TMP,
        H_VIEW_SRC,
        H_FS_HORIZONTAL,
        width,
        height,
        &horizontal,
        clamp,
        None,
    );
    draw_pass(
        &mut stream,
        H_SURFACE_DST,
        H_VIEW_TMP,
        H_FS_VERTICAL,
        width,
        height,
        &vertical,
        clamp,
        Some(grade),
    );
    if !submit(gpu, state, &stream) {
        return false;
    }
    let mark = add(&STAT_DRAW_NS, mark);
    if !transfer(
        gpu,
        CMD_TRANSFER_FROM_HOST_3D,
        RES_DST,
        width,
        height,
        stride as u32,
    ) {
        return false;
    }
    let mark = add(&STAT_FROM_NS, mark);
    let result =
        unsafe { core::slice::from_raw_parts(state.dst_backing as usize as *const u8, total) };
    download(result, stride);
    add(&STAT_DOWNLOAD_NS, mark);
    STAT_CALLS.fetch_add(1, Ordering::Relaxed);
    STAT_PIXELS.fetch_add(width as u64 * height as u64, Ordering::Relaxed);
    true
}

pub struct SelfTest {
    pub ok: bool,
    pub sample: [u8; 4],
}

/// Blurs a flat colour block with radius 0 and neutral grading: the result
/// must be the same colour, which proves the whole pipeline end to end.
pub fn self_test(gpu: &mut GpuState, state: &State) -> SelfTest {
    let side = 16u32.min(state.width).min(state.height);
    let colour = [50u8, 100, 200, 255]; // B, G, R, A
    let mut sample = [0u8; 4];
    let ok = frost_pass(
        gpu,
        state,
        side,
        side,
        0,
        Grade {
            saturation: 1.0,
            brightness: 1.0,
            tint: [0.0; 4],
        },
        &mut |bytes, stride| {
            for row in 0..side as usize {
                for col in 0..side as usize {
                    let at = row * stride + col * 4;
                    bytes[at..at + 4].copy_from_slice(&colour);
                }
            }
        },
        &mut |bytes, stride| {
            let at = (side as usize / 2) * stride + (side as usize / 2) * 4;
            sample.copy_from_slice(&bytes[at..at + 4]);
        },
    );
    let close = |a: u8, b: u8| a.abs_diff(b) <= 2;
    SelfTest {
        ok: ok && (0..3).all(|index| close(sample[index], colour[index])),
        sample,
    }
}
