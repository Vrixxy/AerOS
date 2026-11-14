#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod desktop;
mod library;
mod sim;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowButtons, WindowId};

use library::{Config, Wallpaper};
use sim::Sim;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    res: [f32; 2],
    time: f32,
    field: f32,
    mouse: [f32; 2],
    par: [f32; 2],
    pal: f32,
    srgb: f32,
    _p: [f32; 2],
    custom: [f32; 4],
}

fn rgb_f32(c: [u8; 3]) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0]
}

struct Args {
    rgb: Option<[u8; 3]>,
    window: bool,
    high_gpu: bool,
    quit: bool,
    list: bool,
    wallpaper: Option<String>,
    snapshot: Option<String>,
    size: (u32, u32),
    cursor: Option<(f32, f32)>,
    time: f32,
}

fn parse_args() -> Args {
    let mut a = Args {
        rgb: None,
        window: false,
        high_gpu: false,
        quit: false,
        list: false,
        wallpaper: None,
        snapshot: None,
        size: (1920, 1080),
        cursor: None,
        time: 10.0,
    };
    let v: Vec<String> = std::env::args().skip(1).collect();
    let f = |s: Option<&String>| s.and_then(|x| x.parse::<f32>().ok()).unwrap_or(0.0);
    let mut i = 0;
    while i < v.len() {
        match v[i].as_str() {
            "--window" => a.window = true,
            "--high-gpu" => a.high_gpu = true,
            "--quit" => a.quit = true,
            "--list" => a.list = true,
            "--wallpaper" | "--color" => {
                i += 1;
                a.wallpaper = v.get(i).map(|s| if s.contains('-') { s.clone() } else { format!("silk-{s}") });
            }
            "--snapshot" => {
                i += 1;
                a.snapshot = v.get(i).cloned();
            }
            "--size" => {
                a.size = (v.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(1920), v.get(i + 2).and_then(|s| s.parse().ok()).unwrap_or(1080));
                i += 2;
            }
            "--cursor" => {
                a.cursor = Some((f(v.get(i + 1)), f(v.get(i + 2))));
                i += 2;
            }
            "--time" => {
                a.time = f(v.get(i + 1));
                i += 1;
            }
            "--rgb" => {
                a.rgb = Some([f(v.get(i + 1)) as u8, f(v.get(i + 2)) as u8, f(v.get(i + 3)) as u8]);
                i += 3;
            }
            _ => {}
        }
        i += 1;
    }
    a
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    bgl: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    pipeline: wgpu::RenderPipeline,
}

struct Target {
    ubuf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl Gpu {
    fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat, src: &str) -> Self {
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[&bgl], push_constant_ranges: &[] });
        let mut g = Self { pipeline: placeholder(&device, &layout, format), device, queue, format, bgl, layout };
        g.pipeline = g.build(src).expect("built-in shader failed");
        g
    }

    fn build(&self, src: &str) -> Result<wgpu::RenderPipeline, String> {
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(src.into()) });
        let p = make_pipeline(&self.device, &self.layout, self.format, &module);
        match pollster::block_on(self.device.pop_error_scope()) {
            Some(e) => Err(e.to_string()),
            None => Ok(p),
        }
    }

    fn target(&self) -> Target {
        let ubuf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });
        Target { ubuf, bind_group }
    }

    fn draw(&self, t: &Target, view: &wgpu::TextureView, u: &Uniforms) {
        self.queue.write_buffer(&t.ubuf, 0, bytemuck::bytes_of(u));
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &t.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(Some(enc.finish()));
    }
}

fn make_pipeline(device: &wgpu::Device, layout: &wgpu::PipelineLayout, format: wgpu::TextureFormat, module: &wgpu::ShaderModule) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: Some(layout),
        vertex: wgpu::VertexState { module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

fn placeholder(device: &wgpu::Device, layout: &wgpu::PipelineLayout, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    let src = format!("{}\nfn shade(f: vec4<f32>) -> vec4<f32> {{ return vec4<f32>(0.0, 0.0, 0.0, 1.0); }}", library::PRELUDE);
    let m = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(src.into()) });
    make_pipeline(device, layout, format, &m)
}

fn power(high: bool) -> wgpu::PowerPreference {
    if high { wgpu::PowerPreference::HighPerformance } else { wgpu::PowerPreference::LowPower }
}

fn find_wallpaper(list: &[Wallpaper], id: &str) -> Wallpaper {
    list.iter().find(|w| w.id == id).cloned().unwrap_or_else(|| list[0].clone())
}

fn snapshot(a: &Args, path: &str) {
    let list = library::scan();
    let wp = find_wallpaper(&list, a.wallpaper.as_deref().unwrap_or("silk-purple"));
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: power(true), compatible_surface: None, force_fallback_adapter: false })).expect("no GPU adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).expect("device");
    let (w, h) = a.size;
    let fmt = wgpu::TextureFormat::Rgba8Unorm;
    let gpu = Gpu::new(device, queue, fmt, &library::source(&wp).expect("source"));
    let t = gpu.target();
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: fmt,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    let shift = 0.5 * (w as f32 / h as f32 - 1.7777778);
    let mut u = Uniforms { res: [w as f32, h as f32], time: a.time, field: 0.0, mouse: [0.89, 0.5], par: [0.0, 0.0], pal: library::palette_index(&wp), srgb: 0.0, _p: [0.0; 2], custom: rgb_f32(a.rgb.unwrap_or(library::load_config().custom)) };
    if let Some((cx, cy)) = a.cursor {
        u.field = 1.0;
        u.mouse = [cx / h as f32 - shift, cy / h as f32];
        u.par = [(cx / w as f32 - 0.5) * 2.0, (cy / h as f32 - 0.5) * 2.0];
    }
    gpu.draw(&t, &view, &u);
    let bpr = (w * 4 + 255) / 256 * 256;
    let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (bpr * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    gpu.queue.submit(Some(enc.finish()));
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    gpu.device.poll(wgpu::Maintain::Wait);
    let data = slice.get_mapped_range();
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h as usize {
        for px in data[y * bpr as usize..y * bpr as usize + (w * 4) as usize].chunks_exact(4) {
            rgb.extend_from_slice(&px[..3]);
        }
    }
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().expect("png header").write_image_data(&rgb).expect("png data");
}

static FIRST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Screen {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    mon: desktop::Monitor,
    sim: Sim,
    target: Target,
    covered: bool,
    halted: bool,
    minimized_since: Option<Instant>,
    last_frame: Instant,
}

#[derive(Clone, Copy)]
enum Action {
    Select(usize),
    TogglePause,
    ToggleFullscreen,
    ToggleSaver,
    ToggleStartup,
    PickColour,
    OpenFolder,
    Reload,
    Quit,
}

struct Running {
    gpu: Gpu,
    screens: Vec<Screen>,
    srgb: bool,
    wallpaper_mode: bool,
    tray: TrayIcon,
    actions: HashMap<MenuId, Action>,
    list: Vec<Wallpaper>,
    current: Wallpaper,
    cfg: Config,
    start: Instant,
    last: Instant,
    last_check: Instant,
    saver: bool,
    picker: Option<std::sync::mpsc::Receiver<Option<[u8; 3]>>>,
    before: Option<(Wallpaper, [u8; 3])>,
}

struct App {
    args: Args,
    st: Option<Running>,
}

fn tray_icon() -> Icon {
    let n = 32usize;
    let mut px = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
            let r = (dx * dx + dy * dy).sqrt();
            if r <= 14.5 {
                let t = (y as f32 / n as f32).clamp(0.0, 1.0);
                let i = (y * n + x) * 4;
                px[i] = (90.0 + 110.0 * t) as u8;
                px[i + 1] = (50.0 + 60.0 * t) as u8;
                px[i + 2] = (170.0 + 70.0 * t) as u8;
                px[i + 3] = 255;
            }
        }
    }
    Icon::from_rgba(px, n as u32, n as u32).expect("icon")
}

fn build_menu(list: &[Wallpaper], current: &str, cfg: &Config) -> (Menu, HashMap<MenuId, Action>) {
    let menu = Menu::new();
    let mut map = HashMap::new();
    let sub = Submenu::new("Wallpapers", true);
    for (i, w) in list.iter().enumerate() {
        let it = CheckMenuItem::new(&w.title, true, w.id == current, None);
        map.insert(it.id().clone(), Action::Select(i));
        let _ = sub.append(&it);
    }
    let _ = menu.append(&sub);
    let pick = MenuItem::new("Pick a colour\u{2026}", true, None);
    map.insert(pick.id().clone(), Action::PickColour);
    let _ = menu.append(&pick);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let add_check = |text: &str, on: bool, a: Action, map: &mut HashMap<MenuId, Action>| {
        let it = CheckMenuItem::new(text, true, on, None);
        map.insert(it.id().clone(), a);
        let _ = menu.append(&it);
    };
    add_check("Pause wallpaper", cfg.paused, Action::TogglePause, &mut map);
    add_check("Pause when an app is fullscreen", cfg.pause_fullscreen, Action::ToggleFullscreen, &mut map);
    add_check("Pause in battery saver", cfg.pause_saver, Action::ToggleSaver, &mut map);
    add_check("Run at startup", desktop::startup_enabled(), Action::ToggleStartup, &mut map);
    let _ = menu.append(&PredefinedMenuItem::separator());
    for (text, a) in [("Open wallpaper folder", Action::OpenFolder), ("Reload library", Action::Reload)] {
        let it = MenuItem::new(text, true, None);
        map.insert(it.id().clone(), a);
        let _ = menu.append(&it);
    }
    let _ = menu.append(&PredefinedMenuItem::separator());
    let q = MenuItem::new("Quit", true, None);
    map.insert(q.id().clone(), Action::Quit);
    let _ = menu.append(&q);
    (menu, map)
}

impl App {
    fn create(&mut self, el: &ActiveEventLoop) {
        let wallpaper_mode = !self.args.window;
        library::ensure_example();
        let list = library::scan();
        let mut cfg = library::load_config();
        if let Some(id) = &self.args.wallpaper {
            cfg.wallpaper = id.clone();
        }
        let current = find_wallpaper(&list, &cfg.wallpaper);
        cfg.wallpaper = current.id.clone();
        library::save_config(&cfg);

        let mons: Vec<desktop::Monitor> = if wallpaper_mode { desktop::monitors() } else { vec![desktop::Monitor { x: 0, y: 0, w: 1280, h: 720 }] };
        let mut windows = Vec::new();
        for m in &mons {
            let mut attrs = Window::default_attributes().with_title(desktop::TITLE);
            if wallpaper_mode {
                attrs = attrs.with_decorations(false).with_enabled_buttons(WindowButtons::CLOSE).with_visible(false).with_active(false).with_inner_size(PhysicalSize::new(m.w, m.h)).with_position(PhysicalPosition::new(m.x, m.y));
            } else {
                attrs = attrs.with_inner_size(PhysicalSize::new(m.w, m.h));
            }
            let win = Arc::new(el.create_window(attrs).expect("window"));
            if !wallpaper_mode {
                win.set_visible(true);
            }
            if wallpaper_mode && !desktop::attach_behind_icons(&win, *m) {
                eprintln!("could not attach behind the desktop icons");
            }
            windows.push(win);
        }

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor { backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::DX12), ..Default::default() });
        let surfaces: Vec<wgpu::Surface<'static>> = windows.iter().map(|w| instance.create_surface(w.clone()).expect("surface")).collect();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: power(self.args.high_gpu), compatible_surface: Some(&surfaces[0]), force_fallback_adapter: false })).expect("no GPU adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).expect("device");
        let caps = surfaces[0].get_capabilities(&adapter);
        // a composited (non-opaque) swap chain keeps Windows from treating the wallpaper as a fullscreen app
        let alpha = [wgpu::CompositeAlphaMode::PreMultiplied, wgpu::CompositeAlphaMode::PostMultiplied].into_iter().find(|a| caps.alpha_modes.contains(a)).unwrap_or(caps.alpha_modes[0]);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let src = library::source(&current).unwrap_or_default();
        let mut gpu = Gpu::new(device, queue, format, &library::source(&find_wallpaper(&list, "silk-purple")).unwrap());
        if let Ok(p) = gpu.build(&src) {
            gpu.pipeline = p;
        }

        let mut screens = Vec::new();
        for ((win, surface), m) in windows.into_iter().zip(surfaces).zip(mons) {
            let size = win.inner_size();
            let config = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: size.width.max(1),
                height: size.height.max(1),
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 1,
                alpha_mode: alpha,
                view_formats: vec![],
            };
            surface.configure(&gpu.device, &config);
            let target = gpu.target();
            screens.push(Screen { window: win, surface, config, mon: m, sim: Sim::new(), target, covered: false, halted: false, minimized_since: None, last_frame: Instant::now() });
        }

        std::fs::write(library::root().join("alpha.txt"), format!("{:?} -> {:?} sizes={:?}", caps.alpha_modes, alpha, screens.iter().map(|s| (s.config.width, s.config.height, s.window.is_visible())).collect::<Vec<_>>())).ok();
        let (menu, actions) = build_menu(&list, &current.id, &cfg);
        let tray = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("Animated Wallpapers").with_icon(tray_icon()).build().expect("tray icon");
        let now = Instant::now();
        self.st = Some(Running { gpu, screens, srgb: format.is_srgb(), wallpaper_mode, tray, actions, list, current, cfg, start: now, last: now, last_check: now - Duration::from_secs(1), saver: false, picker: None, before: None });
    }
}

impl Running {
    fn rebuild_menu(&mut self) {
        let (menu, actions) = build_menu(&self.list, &self.current.id, &self.cfg);
        self.tray.set_menu(Some(Box::new(menu)));
        self.actions = actions;
    }

    fn select(&mut self, w: Wallpaper) {
        match library::source(&w).and_then(|s| self.gpu.build(&s)) {
            Ok(p) => {
                self.gpu.pipeline = p;
                self.current = w;
                self.cfg.wallpaper = self.current.id.clone();
                library::save_config(&self.cfg);
                for s in &mut self.screens {
                    s.sim.wake();
                }
            }
            Err(e) => eprintln!("wallpaper failed to load: {e}"),
        }
    }

    fn act(&mut self, a: Action, el: &ActiveEventLoop) {
        match a {
            Action::Select(i) => {
                if let Some(w) = self.list.get(i).cloned() {
                    self.select(w);
                }
            }
            Action::TogglePause => self.cfg.paused = !self.cfg.paused,
            Action::ToggleFullscreen => self.cfg.pause_fullscreen = !self.cfg.pause_fullscreen,
            Action::ToggleSaver => self.cfg.pause_saver = !self.cfg.pause_saver,
            Action::ToggleStartup => desktop::set_startup(!desktop::startup_enabled()),
            Action::PickColour => {
                if self.picker.is_none() {
                    let initial = self.cfg.custom;
                    self.before = Some((self.current.clone(), initial));
                    let custom = find_wallpaper(&self.list, "silk-custom");
                    if self.current.id != custom.id {
                        self.select(custom);
                    }
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = tx.send(desktop::pick_colour(initial));
                    });
                    self.picker = Some(rx);
                }
            }
            Action::OpenFolder => desktop::open_folder(&library::wallpapers_dir()),
            Action::Reload => {
                self.list = library::scan();
                let cur = find_wallpaper(&self.list, &self.cfg.wallpaper);
                self.select(cur);
            }
            Action::Quit => {
                el.exit();
                return;
            }
        }
        library::save_config(&self.cfg);
        self.rebuild_menu();
    }

    fn uniforms(&self, s: &Screen) -> Uniforms {
        Uniforms {
            res: [s.config.width as f32, s.config.height as f32],
            time: self.start.elapsed().as_secs_f32(),
            field: s.sim.field,
            mouse: s.sim.mouse,
            par: s.sim.par,
            pal: library::palette_index(&self.current),
            srgb: if self.srgb { 1.0 } else { 0.0 },
            _p: [0.0; 2],
            custom: rgb_f32(self.cfg.custom),
        }
    }

    fn render(&mut self, idx: usize) {
        let u = self.uniforms(&self.screens[idx]);
        let s = &mut self.screens[idx];
        let frame = match s.surface.get_current_texture() {
            Ok(f) => f,
            Err(_) => {
                s.surface.configure(&self.gpu.device, &s.config);
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        self.gpu.draw(&s.target, &view, &u);
        frame.present();
        if !FIRST.swap(true, std::sync::atomic::Ordering::Relaxed) {
            let _ = std::fs::write(library::root().join("first_frame.txt"), format!("presented {}x{}", s.config.width, s.config.height));
        }
        s.last_frame = Instant::now();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.st.is_none() {
            self.create(el);
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, ev: WindowEvent) {
        let Some(st) = self.st.as_mut() else { return };
        let Some(idx) = st.screens.iter().position(|s| s.window.id() == id) else { return };
        match ev {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(sz) => {
                if sz.width > 0 && sz.height > 0 {
                    let s = &mut st.screens[idx];
                    s.config.width = sz.width;
                    s.config.height = sz.height;
                    s.surface.configure(&st.gpu.device, &s.config);
                    s.sim.wake();
                    s.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => st.render(idx),
            WindowEvent::KeyboardInput { event, .. } => {
                if !st.wallpaper_mode && event.state.is_pressed() && matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                    el.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let Some(st) = self.st.as_mut() else { return };
        let now = Instant::now();
        let dt = (now - st.last).as_secs_f32().min(0.1);
        st.last = now;

        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if let Some(a) = st.actions.get(&ev.id).copied() {
                st.act(a, el);
            }
        }

        if st.picker.is_some() {
            if let Some(c) = desktop::preview() {
                if c != st.cfg.custom {
                    st.cfg.custom = c;
                    for s in &mut st.screens {
                        s.sim.wake();
                    }
                }
            }
        }
        let picked = st.picker.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(res) = picked {
            st.picker = None;
            let before = st.before.take();
            match res {
                Some(c) => st.cfg.custom = c,
                None => {
                    if let Some((w, c)) = before {
                        st.cfg.custom = c;
                        if w.id != st.current.id {
                            st.select(w);
                        }
                    }
                }
            }
            for s in &mut st.screens {
                s.sim.wake();
            }
            library::save_config(&st.cfg);
            st.rebuild_menu();
        }

        if now - st.last_check >= Duration::from_millis(300) {
            st.last_check = now;
            st.saver = st.cfg.pause_saver && desktop::battery_saver_on();
            let fs = st.cfg.pause_fullscreen && st.wallpaper_mode;
            for s in &mut st.screens {
                s.covered = fs && desktop::covered_by_fullscreen(s.mon);
            }
        }

        let mut minimized = false;
        if st.wallpaper_mode && !desktop::embedded() {
            desktop::keep_above_desktop(&st.screens.iter().map(|s| &*s.window).collect::<Vec<_>>());
            for s in &mut st.screens {
                if desktop::is_minimized(&s.window) {
                    minimized = true;
                    let since = *s.minimized_since.get_or_insert(now);
                    if now - since >= Duration::from_millis(200) {
                        desktop::restore_minimized(&s.window, s.mon);
                        s.minimized_since = None;
                        s.sim.wake();
                    }
                } else {
                    s.minimized_since = None;
                }
            }
        }

        let stopped = st.cfg.paused || st.saver;
        let cursor = desktop::cursor_pos();
        let mut moving = false;
        let mut animated = false;
        let mut any_running = false;
        for i in 0..st.screens.len() {
            let halt = stopped || st.screens[i].covered;
            let s = &mut st.screens[i];
            if halt {
                s.halted = true;
                continue;
            }
            any_running = true;
            if s.halted {
                s.halted = false;
                s.sim.wake();
            }
            let rel = cursor.map(|(cx, cy)| {
                if st.wallpaper_mode {
                    ((cx - s.mon.x) as f32, (cy - s.mon.y) as f32)
                } else {
                    let p = s.window.inner_position().unwrap_or_default();
                    ((cx - p.x) as f32, (cy - p.y) as f32)
                }
            });
            let active = s.sim.update(dt, rel, (s.config.width as f32, s.config.height as f32));
            let due = st.current.animated && now - s.last_frame >= Duration::from_millis(33);
            if active || due {
                s.window.request_redraw();
            }
            moving |= active;
            animated |= st.current.animated;
        }

        let wait = if minimized {
            40
        } else if !any_running {
            400
        } else if moving {
            8
        } else if animated {
            16
        } else {
            50
        };
        el.set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(wait)));
    }
}

fn main() {
    let args = parse_args();
    if args.list {
        library::ensure_example();
        for w in library::scan() {
            println!("{}\t{}", w.id, w.title);
        }
        return;
    }
    if args.quit {
        desktop::close_others();
        return;
    }
    if let Some(path) = args.snapshot.clone() {
        snapshot(&args, &path);
        return;
    }
    desktop::close_others();
    std::thread::sleep(Duration::from_millis(400));
    let el = EventLoop::new().expect("event loop");
    el.set_control_flow(ControlFlow::Wait);
    let mut app = App { args, st: None };
    el.run_app(&mut app).expect("run");
}
