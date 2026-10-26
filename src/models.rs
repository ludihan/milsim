//! Mesh helpers for smoother, less boxy models: bevelled boxes, limbs between joints,
//! tapered cylinders and noise-displaced organic shapes.

use std::f32::consts::FRAC_PI_4;

use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology, VertexAttributeValues},
    prelude::*,
};

fn finish(positions: Vec<[f32; 3]>, normals: Vec<[f32; 3]>, indices: Vec<u32>) -> Mesh {
    let uvs = vec![[0.0f32, 0.0]; positions.len()];
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

/// Grid coordinates along one axis of a rounded box: dense in the bevels, sparse on the flats,
/// spaced so the bevel angles are even.
fn axis_coords(h: f32, r: f32, steps: u32) -> Vec<f32> {
    let inner = h - r;
    let mut v = Vec::new();
    for k in (0..=steps).rev() {
        let phi = k as f32 / steps as f32 * FRAC_PI_4;
        v.push(-(inner + r * phi.tan()));
    }
    for k in 0..=steps {
        let phi = k as f32 / steps as f32 * FRAC_PI_4;
        v.push(inner + r * phi.tan());
    }
    v
}

/// Box of `size` with edges rounded to `radius`.
pub fn rounded_box(size: Vec3, radius: f32) -> Mesh {
    let half = size * 0.5;
    let r = radius.min(half.min_element() * 0.98).max(0.001);
    let inner = half - Vec3::splat(r);
    let steps = 3;
    let faces: [(Vec3, Vec3, Vec3); 6] = [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
    ];
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    let extent = |axis: Vec3| (axis.abs() * half).max_element();
    for (n, u, v) in faces {
        let cu = axis_coords(extent(u), r, steps);
        let cv = axis_coords(extent(v), r, steps);
        let base = positions.len() as u32;
        for &b in &cv {
            for &a in &cu {
                let p = n * extent(n) + u * a + v * b;
                let c = p.clamp(-inner, inner);
                let dir = (p - c).normalize_or(n);
                positions.push((c + dir * r).to_array());
                normals.push(dir.to_array());
            }
        }
        let w = cu.len() as u32;
        for j in 0..cv.len() as u32 - 1 {
            for i in 0..w - 1 {
                let i00 = base + j * w + i;
                let i10 = i00 + 1;
                let i01 = i00 + w;
                let i11 = i01 + 1;
                indices.extend_from_slice(&[i00, i10, i11, i00, i11, i01]);
            }
        }
    }
    finish(positions, normals, indices)
}

pub fn capsule(radius: f32, length: f32) -> Mesh {
    Capsule3d::new(radius, length.max(0.001))
        .mesh()
        .latitudes(10)
        .longitudes(14)
        .build()
}

pub fn cylinder(radius: f32, height: f32, resolution: u32) -> Mesh {
    Cylinder::new(radius, height).mesh().resolution(resolution).build()
}

/// Cylinder that narrows from `bottom` to `top` radius, centred on the origin along +Y.
pub fn tapered(bottom: f32, top: f32, height: f32, resolution: u32) -> Mesh {
    ConicalFrustum {
        radius_top: top,
        radius_bottom: bottom,
        height,
    }
    .mesh()
    .resolution(resolution)
    .build()
}

/// Places a +Y-aligned mesh (built with the right length) so its axis runs from `a` to `b`.
pub fn between(a: Vec3, b: Vec3) -> Transform {
    let d = b - a;
    Transform {
        translation: (a + b) * 0.5,
        rotation: Quat::from_rotation_arc(Vec3::Y, d.normalize_or(Vec3::Y)),
        scale: Vec3::ONE,
    }
}

/// A capsule limb running from joint `a` to joint `b`.
pub fn limb(radius: f32, a: Vec3, b: Vec3) -> (Mesh, Transform) {
    let len = (a.distance(b) - 2.0 * radius).max(0.01);
    (capsule(radius, len), between(a, b))
}

fn hash3(p: Vec3, seed: u32) -> f32 {
    let mut h = (p.x * 73.1).to_bits() ^ (p.y * 151.7).to_bits().rotate_left(11)
        ^ (p.z * 37.3).to_bits().rotate_left(22)
        ^ seed.wrapping_mul(0x9E37_79B9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFFFF) as f32 / 65535.0
}

/// Icosphere with each vertex pushed in or out by `amount`, flat shaded (rocks, foliage clumps).
pub fn lumpy(radius: f32, amount: f32, subdivisions: u32, seed: u32) -> Mesh {
    let mut mesh = Sphere::new(radius)
        .mesh()
        .ico(subdivisions)
        .expect("valid icosphere");
    if let Some(VertexAttributeValues::Float32x3(pos)) = mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION) {
        for p in pos.iter_mut() {
            let v = Vec3::from_array(*p);
            let k = 1.0 + (hash3(v.normalize() * 10.0, seed) - 0.5) * 2.0 * amount;
            *p = (v * k).to_array();
        }
    }
    mesh.duplicate_vertices();
    mesh.compute_flat_normals();
    mesh
}

/// Upper hemisphere of `radius` (helmets, canopies), open at the bottom.
pub fn dome(radius: f32, rings: u32, segments: u32) -> Mesh {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    for i in 0..=rings {
        let theta = i as f32 / rings as f32 * std::f32::consts::FRAC_PI_2;
        for j in 0..=segments {
            let phi = j as f32 / segments as f32 * std::f32::consts::TAU;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            positions.push((n * radius).to_array());
            normals.push(n.to_array());
        }
    }
    let w = segments + 1;
    for i in 0..rings {
        for j in 0..segments {
            let a = i * w + j;
            let b = a + w;
            indices.extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
        }
    }
    finish(positions, normals, indices)
}

/// Largest distance of any vertex from the mesh origin along an axis (meshes here are centred).
pub fn half_extent(mesh: &Mesh) -> f32 {
    match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(pos)) => pos
            .iter()
            .map(|p| p[0].abs().max(p[1].abs()).max(p[2].abs()))
            .fold(0.0, f32::max),
        _ => 1.0,
    }
}
