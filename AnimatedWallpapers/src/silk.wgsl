
const TAU: f32 = 6.28318530718;
const NORM: f32 = 0.5967;

struct Wave {
    v0: f32, sl: f32, a1: f32, f1: f32, p1: f32, a2: f32, f2: f32, p2: f32,
};

struct Look {
    base: f32, rim: f32, glow: f32, bodyw: f32, shadow: f32, shw: f32, tilt: f32,
};

fn rimmod(x: f32, ph: f32) -> f32 {
    return 0.62 + 0.38 * (0.5 + 0.5 * sin(TAU * 0.35 * x + ph));
}

fn wave_dn(w: Wave, x: f32, y: f32) -> f32 {
    let t = x - 0.89;
    let a1 = TAU * w.f1 * x + w.p1;
    let a2 = TAU * w.f2 * x + w.p2;
    let g = w.v0 + w.sl * t + w.a1 * sin(a1) + w.a2 * sin(a2);
    let gp = w.sl + w.a1 * TAU * w.f1 * cos(a1) + w.a2 * TAU * w.f2 * cos(a2);
    return (y - g) / sqrt(1.0 + gp * gp);
}

fn dome_dn(cx: f32, cy: f32, a: f32, b: f32, ang: f32, x: f32, y: f32) -> f32 {
    let c = cos(ang);
    let s = sin(ang);
    let dx = x - cx;
    let dy = y - cy;
    let uu = (c * dx + s * dy) / a;
    let vv = (-s * dx + c * dy) / b;
    let r = sqrt(uu * uu + vv * vv);
    return (1.0 - r) * min(a, b) * 0.9;
}

fn paint(L: f32, dn: f32, x: f32, look: Look, px: f32) -> f32 {
    let d = -min(dn, 0.0);
    var sh = 1.0 - look.shadow * exp(-d / look.shw) - 0.18 * exp(-d / 0.28);
    sh = select(1.0, clamp(sh, 0.15, 1.0), dn < 0.0);
    let lit = L * sh;
    let inside = max(dn, 0.0);
    var body = look.base * (0.46 + 0.54 * exp(-inside / look.bodyw));
    body = body * (1.0 + look.tilt * (0.5 - x / 1.78));
    let g = look.glow * exp(-inside / 0.06) + 0.05 * look.base / 0.3 * exp(-pow((inside - 0.16) / 0.09, 2.0));
    let rim = look.rim * 0.62 * exp(-inside / (1.7 * px)) + look.rim * 0.10 * exp(-inside / (6.0 * px));
    let layer = body + g + rim;
    let a = smoothstep(-0.7 * px, 0.7 * px, dn);
    return mix(lit, layer, a);
}

fn ramp(l: f32, c0: vec3<f32>, c1: vec3<f32>, c3: vec3<f32>, t1: f32, t2: f32) -> vec3<f32> {
    let c2 = mix(c1, c3, 0.5);
    if (l < t1) {
        return mix(c0, c1, l / t1);
    }
    if (l < t2) {
        return mix(c1, c2, (l - t1) / (t2 - t1));
    }
    return mix(c2, c3, (l - t2) / (1.0 - t2));
}

fn ramp4(l: f32, c0: vec3<f32>, c1: vec3<f32>, c2: vec3<f32>, c3: vec3<f32>, t1: f32, t2: f32) -> vec3<f32> {
    if (l < t1) {
        return mix(c0, c1, l / t1);
    }
    if (l < t2) {
        return mix(c1, c2, (l - t1) / (t2 - t1));
    }
    return mix(c2, c3, (l - t2) / (1.0 - t2));
}

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(0.1031, 0.1030));
    q = q + dot(q, q.yx + 33.33);
    return fract((q.x + q.y) * q.x);
}

fn shade(frag: vec4<f32>) -> vec4<f32> {
    let H = u.res.y;
    let px = 1.0 / H;
    let aspect = u.res.x / u.res.y;
    let shift = 0.5 * (aspect - 1.7777778);
    let SX = frag.x / H - shift;
    let SY = frag.y / H;

    // the cursor acts like a magnet of the same pole: the fabric is pushed away from it
    let dmv = vec2<f32>(SX, SY) - u.mouse;
    let fall = exp(-dot(dmv, dmv) / 0.10);
    let push = 0.62 * u.field * fall;
    let X0 = SX - dmv.x * push;
    let Y0 = SY - dmv.y * push;

    var L = 0.10 + 0.05 * (1.0 - X0 / 1.78);
    let pk = vec2<f32>(0.040, 0.025);
    var d = 0.20;
    var X = X0 + u.par.x * pk.x * d;
    var Y = Y0 + u.par.y * pk.y * d;
    L = paint(L, wave_dn(Wave(0.12, -0.22, 0.120, 0.46, 0.3, 0.022, 1.3, 1.0), X, Y), X,
        Look(0.42, rimmod(X, 0.2), 0.22, 0.30, 0.55, 0.08, 0.30), px);
    d = 0.30;
    X = X0 + u.par.x * pk.x * d;
    Y = Y0 + u.par.y * pk.y * d;
    L = paint(L, wave_dn(Wave(0.33, -0.30, 0.140, 0.44, 1.5, 0.026, 1.2, 2.2), X, Y), X,
        Look(0.36, rimmod(X, 1.0), 0.22, 0.30, 0.55, 0.08, 0.25), px);
    d = 0.45;
    X = X0 + u.par.x * pk.x * d;
    Y = Y0 + u.par.y * pk.y * d;
    L = paint(L, dome_dn(1.26, 0.44, 0.40, 0.29, -0.55, X, Y), X,
        Look(0.44, rimmod(X, 2.0), 0.20, 0.20, 0.50, 0.08, 0.0), px);
    d = 0.58;
    X = X0 + u.par.x * pk.x * d;
    Y = Y0 + u.par.y * pk.y * d;
    L = paint(L, dome_dn(1.52, 0.58, 0.26, 0.20, -0.42, X, Y), X,
        Look(0.36, rimmod(X, 2.8), 0.16, 0.17, 0.60, 0.07, 0.0), px);
    d = 0.70;
    X = X0 + u.par.x * pk.x * d;
    Y = Y0 + u.par.y * pk.y * d;
    L = paint(L, wave_dn(Wave(0.58, -0.34, 0.130, 0.47, 2.2, 0.026, 1.1, 0.6), X, Y), X,
        Look(0.31, rimmod(X, 1.6), 0.22, 0.30, 0.55, 0.08, 0.15), px);
    d = 0.85;
    X = X0 + u.par.x * pk.x * d;
    Y = Y0 + u.par.y * pk.y * d;
    L = paint(L, wave_dn(Wave(0.76, -0.28, 0.110, 0.50, 0.4, 0.022, 1.3, 1.7), X, Y), X,
        Look(0.25, rimmod(X, 2.4), 0.22, 0.30, 0.55, 0.08, 0.10), px);
    d = 1.00;
    X = X0 + u.par.x * pk.x * d;
    Y = Y0 + u.par.y * pk.y * d;
    L = paint(L, wave_dn(Wave(0.94, -0.22, 0.080, 0.58, 1.3, 0.018, 1.5, 0.3), X, Y), X,
        Look(0.20, rimmod(X, 0.7), 0.15, 0.30, 0.55, 0.08, 0.05), px);

    let vig = 1.0 - 0.30 * clamp(pow((SX - 0.89) / 1.1, 2.0) + pow((SY - 0.5) / 0.75, 2.0), 0.0, 1.4);
    let bloom = 0.10 * exp(-(pow((SX - 0.25) / 0.55, 2.0) + pow((SY - 0.12) / 0.35, 2.0)));
    L = L * vig + bloom * (1.0 - smoothstep(0.12, 0.34, L));

    // light that follows the cursor
    L = L + 0.035 * u.field * exp(-dot(dmv, dmv) / 0.05);

    let Ln = clamp(L / NORM, 0.0, 1.0);
    var col: vec3<f32>;
    var tint: vec3<f32>;
    let p = i32(u.pal + 0.5);
    if (p == 0) {
        col = ramp(pow(Ln, 1.1), vec3<f32>(3.0, 3.0, 6.0), vec3<f32>(38.0, 39.0, 46.0), vec3<f32>(158.0, 160.0, 174.0), 0.5, 0.75);
        tint = vec3<f32>(20.0, 12.0, 40.0);
    } else if (p == 1) {
        col = ramp4(pow(Ln, 0.85), vec3<f32>(16.0, 5.0, 36.0), vec3<f32>(68.0, 26.0, 118.0), vec3<f32>(142.0, 82.0, 204.0), vec3<f32>(232.0, 198.0, 255.0), 0.35, 0.7);
        tint = vec3<f32>(90.0, 30.0, 110.0);
    } else if (p == 2) {
        col = ramp4(pow(Ln, 0.7), vec3<f32>(172.0, 153.0, 128.0), vec3<f32>(212.0, 196.0, 172.0), vec3<f32>(240.0, 230.0, 212.0), vec3<f32>(254.0, 251.0, 244.0), 0.4, 0.75);
        tint = vec3<f32>(-6.0, -10.0, -20.0);
    } else if (p == 3) {
        col = ramp4(pow(Ln, 0.85), vec3<f32>(14.0, 28.0, 64.0), vec3<f32>(62.0, 100.0, 170.0), vec3<f32>(112.0, 152.0, 216.0), vec3<f32>(212.0, 228.0, 252.0), 0.35, 0.7);
        tint = vec3<f32>(80.0, 30.0, 90.0);
    } else if (p == 5) {
        let c = u.custom.rgb * 255.0;
        col = ramp4(pow(Ln, 0.85), c * 0.11, c * 0.48, c, mix(c, vec3<f32>(255.0), 0.7), 0.35, 0.7);
        tint = c * 0.3;
    } else {
        col = ramp4(pow(Ln, 0.9), vec3<f32>(12.0, 12.0, 12.0), vec3<f32>(44.0, 44.0, 44.0), vec3<f32>(90.0, 90.0, 92.0), vec3<f32>(180.0, 180.0, 184.0), 0.4, 0.75);
        tint = vec3<f32>(0.0, 0.0, 0.0);
    }
    col = col / 255.0;
    let warm = exp(-(pow((SX - 0.15) / 0.45, 2.0) + pow((SY - 0.95) / 0.40, 2.0)));
    col = col + warm * (tint / 255.0) * (1.0 - Ln) * 0.9;
    col = col + (hash21(frag.xy) - 0.5) * 0.010;
    col = clamp(col, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(col, 1.0);
}
