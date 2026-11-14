use std::fs;
use std::path::PathBuf;

pub const PRELUDE: &str = r#"
struct U {
    res: vec2<f32>,
    time: f32,
    field: f32,
    mouse: vec2<f32>,
    par: vec2<f32>,
    pal: f32,
    srgb: f32,
    _p: vec2<f32>,
    custom: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    var c = shade(frag);
    if (u.srgb > 0.5) {
        c = vec4<f32>(srgb_to_linear(c.rgb), c.a);
    }
    return vec4<f32>(c.rgb, 1.0);
}
"#;

const SILK: &str = include_str!("silk.wgsl");

const EXAMPLE_SHADER: &str = r#"// Example wallpaper. Write a function called `shade` and return a colour.
// Available through `u`:
//   u.res    screen size in pixels
//   u.time   seconds since the app started
//   u.mouse  cursor position (units of screen height, 16:9 centred)
//   u.par    cursor position from -1 to 1 on each axis, eased
//   u.field  0 when the cursor is away, 1 when it is on this screen
fn shade(frag: vec4<f32>) -> vec4<f32> {
    let uv = frag.xy / u.res;
    let t = u.time * 0.12;
    let a = 0.5 + 0.5 * cos(6.28318 * (vec3<f32>(0.00, 0.33, 0.67) + uv.x * 0.5 + uv.y * 0.3 + t));
    let tint = mix(vec3<f32>(0.10, 0.06, 0.22), vec3<f32>(0.45, 0.30, 0.85), a.x);
    let v = 1.0 - 0.45 * length(uv - 0.5 - u.par * 0.04);
    return vec4<f32>(tint * v, 1.0);
}
"#;

#[derive(Clone)]
pub enum Kind {
    Builtin(u32),
    Shader(PathBuf),
}

#[derive(Clone)]
pub struct Wallpaper {
    pub id: String,
    pub title: String,
    pub kind: Kind,
    pub animated: bool,
}

#[derive(Clone)]
pub struct Config {
    pub wallpaper: String,
    pub pause_fullscreen: bool,
    pub pause_saver: bool,
    pub paused: bool,
    pub custom: [u8; 3],
}

impl Default for Config {
    fn default() -> Self {
        Self { wallpaper: "silk-purple".into(), pause_fullscreen: true, pause_saver: true, paused: false, custom: [140, 90, 230] }
    }
}

pub fn root() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("AnimatedWallpapers")
}

pub fn wallpapers_dir() -> PathBuf {
    root().join("wallpapers")
}

fn builtins() -> Vec<Wallpaper> {
    ["black", "purple", "beige", "blue", "grey", "custom"]
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mut t = format!("Silk {n}");
            t[5..6].make_ascii_uppercase();
            Wallpaper { id: format!("silk-{n}"), title: t, kind: Kind::Builtin(i as u32), animated: false }
        })
        .collect()
}

pub fn ensure_example() {
    let dir = wallpapers_dir();
    if dir.exists() {
        return;
    }
    let ex = dir.join("example-gradient");
    if fs::create_dir_all(&ex).is_ok() {
        let _ = fs::write(ex.join("shader.wgsl"), EXAMPLE_SHADER);
        let _ = fs::write(ex.join("info.txt"), "title=Example gradient\nanimated=1\n");
    }
}

/// Default wallpapers followed by every folder in the wallpapers directory that has a shader.wgsl.
pub fn scan() -> Vec<Wallpaper> {
    let mut v = builtins();
    if let Ok(rd) = fs::read_dir(wallpapers_dir()) {
        let mut dirs: Vec<_> = rd.flatten().filter(|e| e.path().is_dir()).collect();
        dirs.sort_by_key(|e| e.file_name());
        for e in dirs {
            let p = e.path().join("shader.wgsl");
            if !p.is_file() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            let mut title = name.clone();
            let mut animated = false;
            if let Ok(info) = fs::read_to_string(e.path().join("info.txt")) {
                for line in info.lines() {
                    if let Some((k, val)) = line.split_once('=') {
                        match k.trim() {
                            "title" => title = val.trim().to_string(),
                            "animated" => animated = matches!(val.trim(), "1" | "true" | "yes"),
                            _ => {}
                        }
                    }
                }
            }
            v.push(Wallpaper { id: format!("user-{name}"), title, kind: Kind::Shader(p), animated });
        }
    }
    v
}

/// Full shader source (prelude plus the wallpaper's own code).
pub fn source(w: &Wallpaper) -> Result<String, String> {
    let body = match &w.kind {
        Kind::Builtin(_) => SILK.to_string(),
        Kind::Shader(p) => fs::read_to_string(p).map_err(|e| e.to_string())?,
    };
    Ok(format!("{PRELUDE}\n{body}"))
}

pub fn palette_index(w: &Wallpaper) -> f32 {
    match w.kind {
        Kind::Builtin(i) => i as f32,
        _ => 0.0,
    }
}

pub fn load_config() -> Config {
    let mut c = Config::default();
    if let Ok(s) = fs::read_to_string(root().join("config.txt")) {
        for line in s.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let b = matches!(v.trim(), "1" | "true");
                match k.trim() {
                    "wallpaper" => c.wallpaper = v.trim().to_string(),
                    "pause_fullscreen" => c.pause_fullscreen = b,
                    "pause_saver" => c.pause_saver = b,
                    "paused" => c.paused = b,
                    "custom" => {
                        let p: Vec<u8> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                        if p.len() == 3 {
                            c.custom = [p[0], p[1], p[2]];
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    c
}

pub fn save_config(c: &Config) {
    let _ = fs::create_dir_all(root());
    let s = format!(
        "wallpaper={}\npause_fullscreen={}\npause_saver={}\npaused={}\ncustom={},{},{}\n",
        c.wallpaper, c.pause_fullscreen as u8, c.pause_saver as u8, c.paused as u8, c.custom[0], c.custom[1], c.custom[2]
    );
    let _ = fs::write(root().join("config.txt"), s);
}
