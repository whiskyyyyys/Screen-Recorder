// OxideStudio GPU Compositor & Filter Shaders (WGSL)

struct SpriteUniforms {
    // [x, y, w, h] in NDC (-1..1) or pixel transform matrix columns
    mvp_col0: vec4<f32>,
    mvp_col1: vec4<f32>,
    mvp_col2: vec4<f32>,
    mvp_col3: vec4<f32>,
    // [u0, v0, u1, v1]
    uv_rect: vec4<f32>,
    // [opacity, 0, 0, 0]
    params: vec4<f32>,
};

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_samp: sampler;
@group(0) @binding(2) var<uniform> sprite_u: SpriteUniforms;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_sprite(@builtin(vertex_index) vid: u32) -> VsOut {
    // Two triangles forming quad [0..1] x [0..1]
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let p = corners[vid];
    let mvp = mat4x4<f32>(
        sprite_u.mvp_col0,
        sprite_u.mvp_col1,
        sprite_u.mvp_col2,
        sprite_u.mvp_col3,
    );
    var out: VsOut;
    out.pos = mvp * vec4<f32>(p, 0.0, 1.0);
    out.uv = mix(sprite_u.uv_rect.xy, sprite_u.uv_rect.zw, p);
    return out;
}

@fragment
fn fs_sprite(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(src_tex, src_samp, in.uv);
    return vec4<f32>(c.rgb, c.a * sprite_u.params.x);
}

// Fullscreen quad vertex shader used for filters, transitions, scaling, and custom WGSL filters
@vertex
fn vs_fullscreen(@builtin(vertex_index) vid: u32) -> VsOut {
    var pos = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 0.0),
    );
    var out: VsOut;
    out.pos = vec4<f32>(pos[vid], 0.0, 1.0);
    out.uv = uvs[vid];
    return out;
}

// Unified GPU Filter Uniforms
struct FilterUniforms {
    // filter_kind:
    // 0=ChromaKey, 1=ColorKey, 2=LumaKey, 3=ColorCorrection, 4=Sharpen, 5=Scroll, 6=MaskBlend, 7=Scale
    mode: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
    // p0: general 4 floats, p1: 4 floats, p2: 4 floats, p3: 4 floats
    p0: vec4<f32>,
    p1: vec4<f32>,
    p2: vec4<f32>,
    p3: vec4<f32>,
};

@group(0) @binding(0) var f_tex: texture_2d<f32>;
@group(0) @binding(1) var f_samp: sampler;
@group(0) @binding(2) var<uniform> fu: FilterUniforms;
@group(0) @binding(3) var mask_tex: texture_2d<f32>;

fn rgb_to_cbcr(rgb: vec3<f32>) -> vec2<f32> {
    let cb = dot(rgb, vec3<f32>(-0.100644, -0.338572, 0.439216)) + 0.501961;
    let cr = dot(rgb, vec3<f32>(0.439216, -0.398942, -0.040274)) + 0.501961;
    return vec2<f32>(cb, cr);
}

fn apply_gamma_contrast_bright(rgba: vec4<f32>, gamma: f32, contrast: f32, brightness: f32, opacity: f32) -> vec4<f32> {
    let g = max(0.05, 1.0 + gamma);
    let c = 1.0 + contrast;
    let rgb = clamp(pow(max(rgba.rgb, vec3<f32>(0.0)), vec3<f32>(g)) * c + vec3<f32>(brightness), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb, clamp(rgba.a * opacity, 0.0, 1.0));
}

fn rotate_hue(rgb: vec3<f32>, angle_rad: f32) -> vec3<f32> {
    let k = vec3<f32>(0.57735, 0.57735, 0.57735);
    let cos_a = cos(angle_rad);
    return rgb * cos_a + cross(k, rgb) * sin(angle_rad) + k * dot(k, rgb) * (1.0 - cos_a);
}

@fragment
fn fs_filter(in: VsOut) -> @location(0) vec4<f32> {
    let tex_size = vec2<f32>(textureDimensions(f_tex));
    let px = 1.0 / max(tex_size, vec2<f32>(1.0));

    if fu.mode == 0u {
        // ChromaKey: p0 = (key_cb, key_cr, similarity, smoothness)
        //            p1 = (spill, opacity, contrast, brightness)
        //            p2 = (gamma, 0, 0, 0)
        var rgba = textureSample(f_tex, f_samp, in.uv);
        let key_cbcr = fu.p0.xy;
        let sim = max(fu.p0.z, 0.001);
        let smooth_v = max(fu.p0.w, 0.001);
        let spill = max(fu.p1.x, 0.001);

        let d_center = distance(key_cbcr, rgb_to_cbcr(rgba.rgb));
        let d0 = distance(key_cbcr, rgb_to_cbcr(textureSample(f_tex, f_samp, in.uv - vec2<f32>(px.x, px.y * 0.5)).rgb));
        let d1 = distance(key_cbcr, rgb_to_cbcr(textureSample(f_tex, f_samp, in.uv + vec2<f32>(px.x, px.y * 0.5)).rgb));
        let d2 = distance(key_cbcr, rgb_to_cbcr(textureSample(f_tex, f_samp, in.uv - vec2<f32>(px.x * 0.5, -px.y)).rgb));
        let d3 = distance(key_cbcr, rgb_to_cbcr(textureSample(f_tex, f_samp, in.uv + vec2<f32>(px.x * 0.5, -px.y)).rgb));
        let box_dist = (d_center + (d0 + d1 + d2 + d3) * 2.0) / 9.0;

        let base_mask = box_dist - sim;
        let alpha_mask = clamp(base_mask / smooth_v, 0.0, 1.0);
        let spill_val = pow(clamp(base_mask / spill, 0.0, 1.0), 1.5);
        let desat = dot(rgba.rgb, vec3<f32>(0.299, 0.587, 0.114));
        rgba = vec4<f32>(mix(vec3<f32>(desat), rgba.rgb, spill_val), rgba.a * alpha_mask);
        return apply_gamma_contrast_bright(rgba, fu.p2.x, fu.p1.z, fu.p1.w, fu.p1.y);
    } else if fu.mode == 1u {
        // ColorKey: p0 = (key_r, key_g, key_b, similarity)
        //           p1 = (smoothness, opacity, contrast, brightness)
        //           p2 = (gamma, 0, 0, 0)
        var rgba = textureSample(f_tex, f_samp, in.uv);
        let dist = distance(fu.p0.xyz, rgba.rgb);
        let alpha_mask = clamp(max(dist - fu.p0.w, 0.0) / max(fu.p1.x, 0.001), 0.0, 1.0);
        rgba.a *= alpha_mask;
        return apply_gamma_contrast_bright(rgba, fu.p2.x, fu.p1.z, fu.p1.w, fu.p1.y);
    } else if fu.mode == 2u {
        // LumaKey: p0 = (luma_min, luma_min_smooth, luma_max, luma_max_smooth)
        var rgba = textureSample(f_tex, f_samp, in.uv);
        let luma = dot(rgba.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        let clo = smoothstep(fu.p0.x, fu.p0.x + max(fu.p0.y, 0.0001), luma);
        let chi = 1.0 - smoothstep(fu.p0.z - max(fu.p0.w, 0.0001), fu.p0.z, luma);
        rgba.a *= clo * chi;
        return rgba;
    } else if fu.mode == 3u {
        // ColorCorrection: p0=(gamma, contrast, brightness, saturation), p1=(hue_rad, opacity, 0, 0), p2=multiply, p3=add
        var rgba = textureSample(f_tex, f_samp, in.uv);
        rgba = apply_gamma_contrast_bright(rgba, fu.p0.x, fu.p0.y, fu.p0.z, fu.p1.y);
        let luma = dot(rgba.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        var rgb = mix(vec3<f32>(luma), rgba.rgb, 1.0 + fu.p0.w);
        if abs(fu.p1.x) > 0.001 {
            rgb = rotate_hue(rgb, fu.p1.x);
        }
        rgb = clamp(rgb * fu.p2.rgb + fu.p3.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        return vec4<f32>(rgb, rgba.a);
    } else if fu.mode == 4u {
        // Sharpen: p0.x = sharpness
        let c = textureSample(f_tex, f_samp, in.uv);
        let n = textureSample(f_tex, f_samp, in.uv + vec2<f32>(0.0, -px.y));
        let s = textureSample(f_tex, f_samp, in.uv + vec2<f32>(0.0,  px.y));
        let w = textureSample(f_tex, f_samp, in.uv + vec2<f32>(-px.x, 0.0));
        let e = textureSample(f_tex, f_samp, in.uv + vec2<f32>( px.x, 0.0));
        let blur = (n.rgb + s.rgb + w.rgb + e.rgb) * 0.25;
        let sharp = clamp(c.rgb + (c.rgb - blur) * (fu.p0.x * 4.0), vec3<f32>(0.0), vec3<f32>(1.0));
        return vec4<f32>(sharp, c.a);
    } else if fu.mode == 5u {
        // Scroll: p0 = (offset_u, offset_v, loop_flag, 0)
        var uv = in.uv + fu.p0.xy;
        if fu.p0.z > 0.5 {
            uv = fract(uv);
        } else if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
            return vec4<f32>(0.0, 0.0, 0.0, 0.0);
        }
        return textureSample(f_tex, f_samp, uv);
    } else if fu.mode == 6u {
        // ImageMaskBlend: p0 = (mask_type, opacity, 0, 0), p1 = tint_rgba
        let base = textureSample(f_tex, f_samp, in.uv);
        let m = textureSample(mask_tex, f_samp, in.uv) * fu.p1;
        let mtype = u32(fu.p0.x + 0.5);
        let op = fu.p0.y;
        if mtype == 0u {
            let luma = dot(m.rgb, vec3<f32>(0.299, 0.587, 0.114));
            return vec4<f32>(base.rgb, base.a * mix(1.0, luma, op));
        } else if mtype == 1u {
            return vec4<f32>(base.rgb, base.a * mix(1.0, m.a, op));
        } else if mtype == 2u {
            return vec4<f32>(mix(base.rgb, base.rgb * m.rgb, op), base.a);
        } else if mtype == 3u {
            return vec4<f32>(clamp(base.rgb + m.rgb * op, vec3<f32>(0.0), vec3<f32>(1.0)), base.a);
        } else {
            return vec4<f32>(clamp(base.rgb - m.rgb * op, vec3<f32>(0.0), vec3<f32>(1.0)), base.a);
        }
    } else {
        // mode 7: high-quality Bicubic/Lanczos-style sharpened scale pass (p0.x = sharpen factor)
        let c = textureSample(f_tex, f_samp, in.uv);
        let k = fu.p0.x;
        if k <= 0.001 {
            return c;
        }
        let n = textureSample(f_tex, f_samp, in.uv + vec2<f32>(0.0, -px.y));
        let s = textureSample(f_tex, f_samp, in.uv + vec2<f32>(0.0,  px.y));
        let w = textureSample(f_tex, f_samp, in.uv + vec2<f32>(-px.x, 0.0));
        let e = textureSample(f_tex, f_samp, in.uv + vec2<f32>( px.x, 0.0));
        let avg = (n + s + w + e) * 0.25;
        return clamp(c + (c - avg) * k, vec4<f32>(0.0), vec4<f32>(1.0));
    }
}

// Transition shader: blends Scene A (from) and Scene B (to) with optional luma wipe texture
struct TransitionUniforms {
    // kind: 0=Cut/Fade, 1=Swipe(dir), 2=Slide(dir), 3=FadeToColor, 4=LumaWipe
    kind: u32,
    dir: u32,       // 0=Left, 1=Right, 2=Up, 3=Down
    invert: u32,
    swipe_in: u32,
    progress: f32,
    softness: f32,
    switch_point: f32,
    pad: f32,
    color: vec4<f32>,
};

@group(0) @binding(0) var tex_a: texture_2d<f32>;
@group(0) @binding(1) var tex_b: texture_2d<f32>;
@group(0) @binding(2) var tex_luma: texture_2d<f32>;
@group(0) @binding(3) var t_samp: sampler;
@group(0) @binding(4) var<uniform> tu: TransitionUniforms;

fn dir_vec(dir: u32) -> vec2<f32> {
    if dir == 0u { return vec2<f32>(-1.0, 0.0); }
    if dir == 1u { return vec2<f32>( 1.0, 0.0); }
    if dir == 2u { return vec2<f32>( 0.0,-1.0); }
    return vec2<f32>(0.0, 1.0);
}

@fragment
fn fs_transition(in: VsOut) -> @location(0) vec4<f32> {
    let t = clamp(tu.progress, 0.0, 1.0);
    if tu.kind == 0u {
        let a = textureSample(tex_a, t_samp, in.uv);
        let b = textureSample(tex_b, t_samp, in.uv);
        return mix(a, b, t);
    } else if tu.kind == 1u {
        // Swipe
        let d = dir_vec(tu.dir);
        let uv_b = in.uv - d * (1.0 - t);
        if uv_b.x >= 0.0 && uv_b.x <= 1.0 && uv_b.y >= 0.0 && uv_b.y <= 1.0 {
            return textureSample(tex_b, t_samp, uv_b);
        }
        return textureSample(tex_a, t_samp, in.uv);
    } else if tu.kind == 2u {
        // Slide
        let d = dir_vec(tu.dir);
        let uv_a = in.uv - d * t;
        let uv_b = in.uv + d * (1.0 - t);
        if uv_b.x >= 0.0 && uv_b.x <= 1.0 && uv_b.y >= 0.0 && uv_b.y <= 1.0 {
            return textureSample(tex_b, t_samp, uv_b);
        }
        if uv_a.x >= 0.0 && uv_a.x <= 1.0 && uv_a.y >= 0.0 && uv_a.y <= 1.0 {
            return textureSample(tex_a, t_samp, uv_a);
        }
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    } else if tu.kind == 3u {
        // Fade to Color
        let sp = clamp(tu.switch_point, 0.05, 0.95);
        if t < sp {
            let k = t / sp;
            return mix(textureSample(tex_a, t_samp, in.uv), tu.color, k);
        } else {
            let k = (t - sp) / (1.0 - sp);
            return mix(tu.color, textureSample(tex_b, t_samp, in.uv), k);
        }
    } else {
        // Luma Wipe
        let a = textureSample(tex_a, t_samp, in.uv);
        let b = textureSample(tex_b, t_samp, in.uv);
        var luma = textureSample(tex_luma, t_samp, in.uv).r;
        if tu.invert != 0u {
            luma = 1.0 - luma;
        }
        let s = max(tu.softness, 0.001);
        let a_min = luma * (1.0 - s);
        let k = smoothstep(a_min, a_min + s, t);
        return mix(a, b, k);
    }
}

// Compute shader: Converts RGBA8 Program Output texture directly into packed NV12 bytes on GPU!
struct Nv12Uniforms {
    width: u32,
    height: u32,
    color_space: u32, // 0=Rec709, 1=Rec601, 2=Rec2100
    full_range: u32,
};

@group(0) @binding(0) var nv_src: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> nv12_out: array<u32>;
@group(0) @binding(2) var<uniform> nu: Nv12Uniforms;

fn luma_coeffs() -> vec2<f32> {
    if nu.color_space == 1u { return vec2<f32>(0.299, 0.114); }
    if nu.color_space == 2u { return vec2<f32>(0.2627, 0.0593); }
    return vec2<f32>(0.2126, 0.0722);
}

fn sample_y(x: u32, y: u32) -> u32 {
    let rgb = textureLoad(nv_src, vec2<i32>(i32(x), i32(y)), 0).rgb;
    let k = luma_coeffs();
    let kg = 1.0 - k.x - k.y;
    let luma = k.x * rgb.r + kg * rgb.g + k.y * rgb.b;
    let y_off = select(16.0, 0.0, nu.full_range != 0u);
    let y_scale = select(219.0, 255.0, nu.full_range != 0u);
    return u32(clamp(round(y_off + luma * y_scale), 0.0, 255.0));
}

@compute @workgroup_size(8, 8, 1)
fn cs_rgba_to_nv12(@builtin(global_invocation_id) gid: vec3<u32>) {
    // Each thread writes 4 horizontal Y pixels (aligned to 4 bytes -> 1 u32)
    // and when y is even, writes 2 UV pairs (4 bytes -> 1 u32) if gid.x * 4 < width!
    let x4 = gid.x * 4u;
    let y = gid.y;
    if x4 >= nu.width || y >= nu.height {
        return;
    }

    let y0 = sample_y(x4, y);
    let y1 = sample_y(min(x4 + 1u, nu.width - 1u), y);
    let y2 = sample_y(min(x4 + 2u, nu.width - 1u), y);
    let y3 = sample_y(min(x4 + 3u, nu.width - 1u), y);

    let y_word_idx = (y * nu.width + x4) / 4u;
    nv12_out[y_word_idx] = y0 | (y1 << 8u) | (y2 << 16u) | (y3 << 24u);

    if (y & 1u) == 0u && y + 1u < nu.height {
        let k = luma_coeffs();
        let kg = 1.0 - k.x - k.y;
        let uv_scale = select(224.0, 255.0, nu.full_range != 0u);

        // Pair 0 (x4, x4+1)
        let rgb0 = (
            textureLoad(nv_src, vec2<i32>(i32(x4), i32(y)), 0).rgb +
            textureLoad(nv_src, vec2<i32>(i32(x4 + 1u), i32(y)), 0).rgb +
            textureLoad(nv_src, vec2<i32>(i32(x4), i32(y + 1u)), 0).rgb +
            textureLoad(nv_src, vec2<i32>(i32(x4 + 1u), i32(y + 1u)), 0).rgb
        ) * 0.25;
        let l0 = k.x * rgb0.r + kg * rgb0.g + k.y * rgb0.b;
        let u0 = u32(clamp(round(128.0 + 0.5 * ((rgb0.b - l0) / (1.0 - k.y)) * uv_scale), 0.0, 255.0));
        let v0 = u32(clamp(round(128.0 + 0.5 * ((rgb0.r - l0) / (1.0 - k.x)) * uv_scale), 0.0, 255.0));

        // Pair 1 (x4+2, x4+3)
        let x2 = min(x4 + 2u, nu.width - 2u);
        let rgb1 = (
            textureLoad(nv_src, vec2<i32>(i32(x2), i32(y)), 0).rgb +
            textureLoad(nv_src, vec2<i32>(i32(x2 + 1u), i32(y)), 0).rgb +
            textureLoad(nv_src, vec2<i32>(i32(x2), i32(y + 1u)), 0).rgb +
            textureLoad(nv_src, vec2<i32>(i32(x2 + 1u), i32(y + 1u)), 0).rgb
        ) * 0.25;
        let l1 = k.x * rgb1.r + kg * rgb1.g + k.y * rgb1.b;
        let u1 = u32(clamp(round(128.0 + 0.5 * ((rgb1.b - l1) / (1.0 - k.y)) * uv_scale), 0.0, 255.0));
        let v1 = u32(clamp(round(128.0 + 0.5 * ((rgb1.r - l1) / (1.0 - k.x)) * uv_scale), 0.0, 255.0));

        let uv_base_words = (nu.width * nu.height) / 4u;
        let uv_word_idx = uv_base_words + ((y / 2u) * nu.width + x4) / 4u;
        nv12_out[uv_word_idx] = u0 | (v0 << 8u) | (u1 << 16u) | (v1 << 24u);
    }
}
