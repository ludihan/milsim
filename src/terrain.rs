//! Procedural island: heightfield, locations, roads, terrain meshes and the map image.

use std::sync::OnceLock;

use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

use crate::common::smoothstep;

pub const MAP_HALF: f32 = 2048.0;
pub const GRID: usize = 512;
pub const CELL: f32 = MAP_HALF * 2.0 / GRID as f32;
pub const CHUNKS: usize = 8;
pub const WATER_LEVEL: f32 = 0.0;
pub const MAP_IMAGE_SIZE: u32 = 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LocKind {
    Base,
    Town,
    Village,
    Outpost,
}

pub struct Location {
    pub name: &'static str,
    pub pos: Vec2,
    pub radius: f32,
    pub kind: LocKind,
}

const fn loc(name: &'static str, x: f32, z: f32, radius: f32, kind: LocKind) -> Location {
    Location {
        name,
        pos: Vec2::new(x, z),
        radius,
        kind,
    }
}

pub const BASE: usize = 0;

pub const LOCATIONS: &[Location] = &[
    loc("FOB Anvil", -1350.0, 1150.0, 75.0, LocKind::Base),
    loc("Lipovka", -650.0, 650.0, 95.0, LocKind::Town),
    loc("Novaya Gora", 250.0, 1150.0, 80.0, LocKind::Village),
    loc("Sosnovka", -250.0, 0.0, 75.0, LocKind::Village),
    loc("Belyy Kamen", 650.0, 250.0, 125.0, LocKind::Town),
    loc("Krasnoye", -1000.0, -400.0, 90.0, LocKind::Town),
    loc("Hill 312", 1150.0, 950.0, 50.0, LocKind::Outpost),
    loc("Dubrava", 1250.0, -650.0, 100.0, LocKind::Town),
    loc("Staroye", 50.0, -1000.0, 110.0, LocKind::Town),
];

const ROAD_LINKS: &[(usize, usize)] = &[
    (0, 1),
    (1, 2),
    (1, 3),
    (3, 4),
    (4, 6),
    (2, 6),
    (3, 5),
    (5, 8),
    (8, 7),
    (7, 4),
    (3, 8),
];

/// Road centre-line segments, gently curved between linked locations.
pub fn road_segments() -> &'static [(Vec2, Vec2)] {
    static SEGMENTS: OnceLock<Vec<(Vec2, Vec2)>> = OnceLock::new();
    SEGMENTS.get_or_init(|| {
        let mut segs = Vec::new();
        for (k, &(a, b)) in ROAD_LINKS.iter().enumerate() {
            let pa = LOCATIONS[a].pos;
            let pb = LOCATIONS[b].pos;
            let dir = pb - pa;
            let side = Vec2::new(-dir.y, dir.x).normalize_or_zero();
            let bend = dir.length() * 0.08 * if k % 2 == 0 { 1.0 } else { -1.0 };
            let steps = 10;
            let mut prev = pa;
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                let wobble = (t * std::f32::consts::PI).sin() * bend
                    + (t * 9.0 + k as f32).sin() * 12.0 * (t * std::f32::consts::PI).sin();
                let p = pa + dir * t + side * wobble;
                segs.push((prev, p));
                prev = p;
            }
        }
        segs
    })
}

pub fn road_distance(p: Vec2) -> f32 {
    let mut best = f32::MAX;
    for &(a, b) in road_segments() {
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
        best = best.min(p.distance_squared(a + ab * t));
    }
    best.sqrt()
}

fn hash2(ix: i32, iz: i32, seed: u32) -> f32 {
    let mut h = (ix as u32).wrapping_mul(0x8da6_b343)
        ^ (iz as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0x00ff_ffff) as f32 / 16_777_215.0
}

pub fn noise(x: f32, z: f32, seed: u32) -> f32 {
    let (ix, iz) = (x.floor(), z.floor());
    let (fx, fz) = (x - ix, z - iz);
    let (ux, uz) = (fx * fx * (3.0 - 2.0 * fx), fz * fz * (3.0 - 2.0 * fz));
    let (ix, iz) = (ix as i32, iz as i32);
    let a = hash2(ix, iz, seed);
    let b = hash2(ix + 1, iz, seed);
    let c = hash2(ix, iz + 1, seed);
    let d = hash2(ix + 1, iz + 1, seed);
    let top = a + (b - a) * ux;
    let bottom = c + (d - c) * ux;
    top + (bottom - top) * uz
}

pub fn fbm(x: f32, z: f32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 0.5, 1.0, 0.0);
    for o in 0..octaves {
        sum += noise(x * freq, z * freq, seed + o * 31) * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.03;
    }
    sum / norm
}

/// Height before towns are flattened.
pub fn raw_height(x: f32, z: f32) -> f32 {
    let d = Vec2::new(x, z).length() / MAP_HALF + (fbm(x * 0.0015, z * 0.0015, 3, 7) - 0.5) * 0.18;
    let land = 1.0 - smoothstep(0.82, 1.0, d);
    let hills = fbm(x * 0.0011, z * 0.0011, 5, 1);
    let ridge = 1.0 - (fbm(x * 0.0024, z * 0.0024, 4, 3) * 2.0 - 1.0).abs();
    let detail = fbm(x * 0.012, z * 0.012, 3, 5) - 0.5;
    let h = 7.0 + (hills - 0.38).max(0.0) * 240.0 + ridge.powi(3) * 40.0 * hills + detail * 3.0;
    h * land - 30.0 * (1.0 - land)
}

#[derive(Resource)]
pub struct Terrain {
    heights: Vec<f32>,
}

impl Terrain {
    pub fn generate() -> Self {
        let n = GRID + 1;
        let zone_heights: Vec<f32> = LOCATIONS
            .iter()
            .map(|l| raw_height(l.pos.x, l.pos.y).max(9.0))
            .collect();
        let mut heights = vec![0.0; n * n];
        for j in 0..n {
            for i in 0..n {
                let x = -MAP_HALF + i as f32 * CELL;
                let z = -MAP_HALF + j as f32 * CELL;
                let mut h = raw_height(x, z);
                for (l, zh) in LOCATIONS.iter().zip(&zone_heights) {
                    let d = Vec2::new(x, z).distance(l.pos);
                    let t = smoothstep(l.radius, l.radius + 110.0, d);
                    h = zh + (h - zh) * t;
                }
                heights[j * n + i] = h;
            }
        }
        Self { heights }
    }

    fn at(&self, i: usize, j: usize) -> f32 {
        let n = GRID + 1;
        self.heights[j.min(GRID) * n + i.min(GRID)]
    }

    /// Exact height of the rendered triangle surface.
    pub fn height(&self, x: f32, z: f32) -> f32 {
        let gx = ((x + MAP_HALF) / CELL).clamp(0.0, GRID as f32 - 0.001);
        let gz = ((z + MAP_HALF) / CELL).clamp(0.0, GRID as f32 - 0.001);
        let (i, j) = (gx.floor() as usize, gz.floor() as usize);
        let (fx, fz) = (gx - i as f32, gz - j as f32);
        let h00 = self.at(i, j);
        let h10 = self.at(i + 1, j);
        let h01 = self.at(i, j + 1);
        let h11 = self.at(i + 1, j + 1);
        if fz > fx {
            h00 + (h11 - h01) * fx + (h01 - h00) * fz
        } else {
            h00 + (h10 - h00) * fx + (h11 - h10) * fz
        }
    }

    pub fn normal(&self, x: f32, z: f32) -> Vec3 {
        let e = 1.5;
        let hx = self.height(x + e, z) - self.height(x - e, z);
        let hz = self.height(x, z + e) - self.height(x, z - e);
        Vec3::new(-hx, 2.0 * e, -hz).normalize()
    }

    fn grid_normal(&self, i: usize, j: usize) -> Vec3 {
        let hx = self.at(i + 1, j) - self.at(i.saturating_sub(1), j);
        let hz = self.at(i, j + 1) - self.at(i, j.saturating_sub(1));
        Vec3::new(-hx, 2.0 * CELL, -hz).normalize()
    }

    /// First intersection of segment a->b with the ground, as a fraction of the segment.
    pub fn raycast(&self, a: Vec3, b: Vec3) -> Option<f32> {
        let below = |p: Vec3| p.y < self.height(p.x, p.z);
        if below(a) {
            return Some(0.0);
        }
        let len = a.distance(b);
        let steps = (len / 2.5).ceil().max(1.0) as usize;
        let mut prev = 0.0;
        for s in 1..=steps {
            let t = s as f32 / steps as f32;
            if below(a.lerp(b, t)) {
                let (mut lo, mut hi) = (prev, t);
                for _ in 0..10 {
                    let m = (lo + hi) * 0.5;
                    if below(a.lerp(b, m)) {
                        hi = m;
                    } else {
                        lo = m;
                    }
                }
                return Some(hi);
            }
            prev = t;
        }
        None
    }

    pub fn chunk_mesh(&self, cx: usize, cz: usize) -> Mesh {
        let per = GRID / CHUNKS;
        let (i0, j0) = (cx * per, cz * per);
        let n = per + 1;
        let mut positions = Vec::with_capacity(n * n);
        let mut normals = Vec::with_capacity(n * n);
        let mut colors = Vec::with_capacity(n * n);
        let mut uvs = Vec::with_capacity(n * n);
        for j in 0..n {
            for i in 0..n {
                let (gi, gj) = (i0 + i, j0 + j);
                let x = -MAP_HALF + gi as f32 * CELL;
                let z = -MAP_HALF + gj as f32 * CELL;
                let h = self.at(gi, gj);
                let nrm = self.grid_normal(gi, gj);
                positions.push([x, h, z]);
                normals.push(nrm.to_array());
                colors.push(ground_color(x, z, h, nrm).to_f32_array());
                uvs.push([x / 8.0, z / 8.0]);
            }
        }
        let mut indices = Vec::with_capacity(per * per * 6);
        for j in 0..per {
            for i in 0..per {
                let v00 = (j * n + i) as u32;
                let v10 = v00 + 1;
                let v01 = v00 + n as u32;
                let v11 = v01 + 1;
                indices.extend_from_slice(&[v00, v01, v11, v00, v11, v10]);
            }
        }
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32(indices))
    }
}

/// Distance-weighted "how much of a settlement is here" (0 outside, 1 in the centre).
pub fn settlement_factor(p: Vec2) -> f32 {
    LOCATIONS
        .iter()
        .map(|l| 1.0 - smoothstep(l.radius * 0.7, l.radius + 15.0, p.distance(l.pos)))
        .fold(0.0, f32::max)
}

fn ground_color(x: f32, z: f32, h: f32, n: Vec3) -> LinearRgba {
    let mix = |a: Vec3, b: Vec3, t: f32| a + (b - a) * t.clamp(0.0, 1.0);
    let srgb = |r: f32, g: f32, b: f32| {
        let c = LinearRgba::from(Color::srgb(r, g, b));
        Vec3::new(c.red, c.green, c.blue)
    };
    let grass_a = srgb(0.33, 0.42, 0.2);
    let grass_b = srgb(0.45, 0.47, 0.25);
    let dry = srgb(0.55, 0.52, 0.34);
    let rock = srgb(0.44, 0.42, 0.39);
    let sand = srgb(0.72, 0.66, 0.5);
    let dirt = srgb(0.43, 0.36, 0.27);
    let field = srgb(0.6, 0.55, 0.3);

    let v = fbm(x * 0.015, z * 0.015, 3, 11);
    let mut c = mix(grass_a, grass_b, v * 1.6 - 0.3);
    c = mix(c, dry, smoothstep(0.62, 0.75, fbm(x * 0.004, z * 0.004, 2, 13)));
    // Farm fields: blocky patches near settlements.
    let fx = (x / 60.0).floor();
    let fz = (z / 45.0).floor();
    let near_town = LOCATIONS
        .iter()
        .any(|l| l.kind != LocKind::Base && Vec2::new(x, z).distance(l.pos) < l.radius + 260.0);
    if near_town && hash2(fx as i32, fz as i32, 99) > 0.72 {
        c = mix(c, field, 0.7);
    }
    let slope = 1.0 - n.y;
    c = mix(c, rock, smoothstep(0.18, 0.35, slope));
    c = mix(c, dirt, settlement_factor(Vec2::new(x, z)) * 0.45);
    let rd = road_distance(Vec2::new(x, z));
    c = mix(c, dirt, 1.0 - smoothstep(3.0, 6.5, rd));
    c = mix(c, sand, 1.0 - smoothstep(1.0, 3.0, h));
    if h < WATER_LEVEL {
        c *= 0.7;
    }
    LinearRgba::new(c.x, c.y, c.z, 1.0)
}

/// Building footprint for the map: centre, half size, yaw.
pub struct Footprint {
    pub center: Vec2,
    pub half: Vec2,
    pub yaw: f32,
}

/// Paper-style topographic map, `MAP_IMAGE_SIZE` square, north (-Z) up.
pub fn map_image(terrain: &Terrain, buildings: &[Footprint], trees: &[Vec2]) -> Image {
    let size = MAP_IMAGE_SIZE as usize;
    let mpp = MAP_HALF * 2.0 / size as f32;
    let mut data = vec![0u8; size * size * 4];
    let mut heights = vec![0.0f32; size * size];
    for py in 0..size {
        for px in 0..size {
            let x = -MAP_HALF + (px as f32 + 0.5) * mpp;
            let z = -MAP_HALF + (py as f32 + 0.5) * mpp;
            heights[py * size + px] = terrain.height(x, z);
        }
    }
    let put = |data: &mut Vec<u8>, px: usize, py: usize, c: [f32; 3]| {
        let o = (py * size + px) * 4;
        data[o] = (c[0] * 255.0) as u8;
        data[o + 1] = (c[1] * 255.0) as u8;
        data[o + 2] = (c[2] * 255.0) as u8;
        data[o + 3] = 255;
    };
    for py in 0..size {
        for px in 0..size {
            let h = heights[py * size + px];
            let hr = heights[py * size + (px + 1).min(size - 1)];
            let hd = heights[(py + 1).min(size - 1) * size + px];
            let shade = ((h - hr) * 0.08 + (h - hd) * 0.08).clamp(-0.12, 0.12);
            let mut c = if h < WATER_LEVEL {
                let deep = (-h / 30.0).clamp(0.0, 1.0);
                [0.62 - deep * 0.15, 0.78 - deep * 0.12, 0.9 - deep * 0.05]
            } else {
                let t = (h / 110.0).clamp(0.0, 1.0);
                [
                    0.93 - t * 0.12 + shade,
                    0.92 - t * 0.17 + shade,
                    0.84 - t * 0.2 + shade,
                ]
            };
            // Contour lines every 10 m, thicker every 50 m.
            if h > WATER_LEVEL {
                let band = (h / 10.0).floor();
                if (hr / 10.0).floor() != band || (hd / 10.0).floor() != band {
                    let major = ((band.max((hr / 10.0).floor()).max((hd / 10.0).floor())) as i32)
                        % 5
                        == 0;
                    let k = if major { 0.62 } else { 0.8 };
                    c = [c[0] * k + 0.08, c[1] * k + 0.02, c[2] * k - 0.04];
                }
            } else if (hr >= WATER_LEVEL) != (h >= WATER_LEVEL)
                || (hd >= WATER_LEVEL) != (h >= WATER_LEVEL)
            {
                c = [0.3, 0.45, 0.6];
            }
            put(&mut data, px, py, c.map(|v| v.clamp(0.0, 1.0)));
        }
    }
    let to_px = |p: Vec2| {
        (
            ((p.x + MAP_HALF) / mpp) as i32,
            ((p.y + MAP_HALF) / mpp) as i32,
        )
    };
    let blend = |data: &mut Vec<u8>, px: i32, py: i32, c: [f32; 3], a: f32| {
        if px < 0 || py < 0 || px >= size as i32 || py >= size as i32 {
            return;
        }
        let o = (py as usize * size + px as usize) * 4;
        for k in 0..3 {
            let old = data[o + k] as f32 / 255.0;
            data[o + k] = ((old + (c[k] - old) * a) * 255.0) as u8;
        }
    };
    for &t in trees {
        let (px, py) = to_px(t);
        blend(&mut data, px, py, [0.45, 0.7, 0.4], 0.55);
    }
    // Grid: 100 m minor, 1 km major.
    for i in 0..size {
        let m = -MAP_HALF + i as f32 * mpp;
        let rel = (m + MAP_HALF) % 1000.0;
        let minor = (m + MAP_HALF) % 100.0;
        let (on_major, on_minor) = (rel < mpp, minor < mpp);
        if on_major || on_minor {
            let a = if on_major { 0.45 } else { 0.12 };
            for k in 0..size {
                blend(&mut data, i as i32, k as i32, [0.2, 0.2, 0.3], a);
                blend(&mut data, k as i32, i as i32, [0.2, 0.2, 0.3], a);
            }
        }
    }
    for &(a, b) in road_segments() {
        let len = a.distance(b);
        let steps = (len / (mpp * 0.5)) as usize + 1;
        for s in 0..=steps {
            let p = a.lerp(b, s as f32 / steps as f32);
            let (px, py) = to_px(p);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                blend(&mut data, px + dx, py + dy, [0.78, 0.45, 0.2], 0.9);
            }
        }
    }
    for b in buildings {
        let r = b.half.length();
        let (cx, cy) = to_px(b.center);
        let rp = (r / mpp).ceil() as i32 + 1;
        let (s, c) = b.yaw.sin_cos();
        for dy in -rp..=rp {
            for dx in -rp..=rp {
                let w = Vec2::new(dx as f32 * mpp, dy as f32 * mpp);
                // Inverse-rotate into the footprint's local frame (yaw around +Y).
                let local = Vec2::new(w.x * c - w.y * s, w.x * s + w.y * c);
                if local.x.abs() <= b.half.x && local.y.abs() <= b.half.y {
                    blend(&mut data, cx + dx, cy + dy, [0.25, 0.22, 0.2], 0.9);
                }
            }
        }
    }
    Image::new(
        Extent3d {
            width: MAP_IMAGE_SIZE,
            height: MAP_IMAGE_SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}
