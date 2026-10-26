//! Static world: obstacle collision grid and procedural props (forests, towns, bases).

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI};

use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::VisibilityRange,
    light::{CascadeShadowConfigBuilder, NotShadowCaster},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

use crate::common::*;
use crate::terrain::*;

pub const MOVE: u8 = 1;
pub const BULLET: u8 = 2;
pub const SIGHT: u8 = 4;
pub const SOLID: u8 = MOVE | BULLET | SIGHT;

#[derive(Clone, Copy, Debug)]
pub enum Shape {
    /// Vertical cylinder; `pos` is the centre of its base.
    Cyl { r: f32, h: f32 },
    /// Box rotated around +Y; `pos` is its centre.
    Obb { half: Vec3, yaw: f32 },
    Sphere { r: f32 },
}

pub struct Obstacle {
    pub pos: Vec3,
    pub shape: Shape,
    pub mask: u8,
    pub entity: Option<Entity>,
    pub active: bool,
    pub tree: bool,
}

impl Obstacle {
    fn radius_xz(&self) -> f32 {
        match self.shape {
            Shape::Cyl { r, .. } | Shape::Sphere { r } => r,
            Shape::Obb { half, .. } => Vec2::new(half.x, half.z).length(),
        }
    }

    pub fn segment(&self, a: Vec3, b: Vec3) -> Option<f32> {
        match self.shape {
            Shape::Sphere { r } => segment_sphere(a, b, self.pos, r),
            Shape::Obb { half, yaw } => {
                let inv = Quat::from_rotation_y(-yaw);
                segment_box_local(inv * (a - self.pos), inv * (b - self.pos), half)
            }
            Shape::Cyl { r, h } => {
                let d = b - a;
                let f = Vec2::new(a.x - self.pos.x, a.z - self.pos.z);
                let dd = Vec2::new(d.x, d.z);
                let aa = dd.length_squared();
                let c = f.length_squared() - r * r;
                if c < 0.0 || aa < 1e-10 {
                    return None;
                }
                let bb = f.dot(dd);
                let disc = bb * bb - aa * c;
                if disc < 0.0 {
                    return None;
                }
                let t = (-bb - disc.sqrt()) / aa;
                if !(0.0..=1.0).contains(&t) {
                    return None;
                }
                let y = a.y + d.y * t;
                (y >= self.pos.y && y <= self.pos.y + h).then_some(t)
            }
        }
    }

    /// Horizontal push that moves a vertical capsule (feet at `p`) out of this obstacle.
    pub fn push(&self, p: Vec3, radius: f32, height: f32) -> Option<Vec2> {
        match self.shape {
            Shape::Cyl { r, h } => {
                if p.y > self.pos.y + h || p.y + height < self.pos.y {
                    return None;
                }
                push_circle(p, self.pos, r + radius)
            }
            Shape::Sphere { r } => {
                if p.y > self.pos.y + r * 0.8 || p.y + height < self.pos.y - r {
                    return None;
                }
                push_circle(p, self.pos, r * 0.85 + radius)
            }
            Shape::Obb { half, yaw } => {
                if p.y > self.pos.y + half.y - 0.35 || p.y + height < self.pos.y - half.y {
                    return None;
                }
                let inv = Quat::from_rotation_y(-yaw);
                let l = inv * (p - self.pos);
                let closest = Vec2::new(l.x.clamp(-half.x, half.x), l.z.clamp(-half.z, half.z));
                let diff = Vec2::new(l.x, l.z) - closest;
                let dist = diff.length();
                let local_push = if dist > 1e-4 {
                    if dist >= radius {
                        return None;
                    }
                    diff / dist * (radius - dist)
                } else {
                    let px = half.x - l.x.abs() + radius;
                    let pz = half.z - l.z.abs() + radius;
                    if px < pz {
                        Vec2::new(px * l.x.signum(), 0.0)
                    } else {
                        Vec2::new(0.0, pz * l.z.signum())
                    }
                };
                let w = Quat::from_rotation_y(yaw) * Vec3::new(local_push.x, 0.0, local_push.y);
                Some(Vec2::new(w.x, w.z))
            }
        }
    }
}

fn push_circle(p: Vec3, c: Vec3, min_dist: f32) -> Option<Vec2> {
    let d = Vec2::new(p.x - c.x, p.z - c.z);
    let len = d.length();
    if len >= min_dist {
        return None;
    }
    let dir = if len > 1e-4 { d / len } else { Vec2::X };
    Some(dir * (min_dist - len))
}

const GCELL: f32 = 16.0;

fn cell_of(v: f32) -> i32 {
    (v / GCELL).floor() as i32
}

#[derive(Resource, Default)]
pub struct Obstacles {
    pub list: Vec<Obstacle>,
    grid: HashMap<(i32, i32), Vec<u32>>,
}

impl Obstacles {
    pub fn add(&mut self, o: Obstacle) -> usize {
        let idx = self.list.len();
        let r = o.radius_xz();
        for cx in cell_of(o.pos.x - r)..=cell_of(o.pos.x + r) {
            for cz in cell_of(o.pos.z - r)..=cell_of(o.pos.z + r) {
                self.grid.entry((cx, cz)).or_default().push(idx as u32);
            }
        }
        self.list.push(o);
        idx
    }

    pub fn for_each_near(&self, p: Vec2, r: f32, mut f: impl FnMut(usize, &Obstacle)) {
        for cx in cell_of(p.x - r)..=cell_of(p.x + r) {
            for cz in cell_of(p.y - r)..=cell_of(p.y + r) {
                if let Some(ids) = self.grid.get(&(cx, cz)) {
                    for &i in ids {
                        let o = &self.list[i as usize];
                        if o.active {
                            f(i as usize, o);
                        }
                    }
                }
            }
        }
    }

    /// Visit the obstacles in every grid cell the segment crosses; stop when `f` returns true.
    fn walk_segment(&self, a: Vec3, b: Vec3, mut f: impl FnMut(usize) -> bool) {
        let (ax, az) = (a.x / GCELL, a.z / GCELL);
        let (bx, bz) = (b.x / GCELL, b.z / GCELL);
        let (mut ix, mut iz) = (ax.floor() as i32, az.floor() as i32);
        let (ex, ez) = (bx.floor() as i32, bz.floor() as i32);
        let (dx, dz) = (bx - ax, bz - az);
        let step_x = if dx > 0.0 { 1 } else { -1 };
        let step_z = if dz > 0.0 { 1 } else { -1 };
        let tdx = if dx != 0.0 { 1.0 / dx.abs() } else { f32::INFINITY };
        let tdz = if dz != 0.0 { 1.0 / dz.abs() } else { f32::INFINITY };
        let mut tmx = if dx > 0.0 {
            (ax.floor() + 1.0 - ax) * tdx
        } else if dx < 0.0 {
            (ax - ax.floor()) * tdx
        } else {
            f32::INFINITY
        };
        let mut tmz = if dz > 0.0 {
            (az.floor() + 1.0 - az) * tdz
        } else if dz < 0.0 {
            (az - az.floor()) * tdz
        } else {
            f32::INFINITY
        };
        let max_iter = (ex - ix).abs() + (ez - iz).abs() + 2;
        for _ in 0..=max_iter {
            if let Some(ids) = self.grid.get(&(ix, iz)) {
                for &i in ids {
                    if f(i as usize) {
                        return;
                    }
                }
            }
            if ix == ex && iz == ez {
                return;
            }
            if tmx < tmz {
                tmx += tdx;
                ix += step_x;
            } else {
                tmz += tdz;
                iz += step_z;
            }
        }
    }

    /// Nearest obstacle hit along a->b with any of the bits in `mask`.
    pub fn segment_hit(&self, a: Vec3, b: Vec3, mask: u8) -> Option<(f32, usize)> {
        let mut best: Option<(f32, usize)> = None;
        self.walk_segment(a, b, |i| {
            let o = &self.list[i];
            if o.active
                && o.mask & mask != 0
                && let Some(t) = o.segment(a, b)
                && best.is_none_or(|(bt, _)| t < bt)
            {
                best = Some((t, i));
            }
            false
        });
        best
    }

    pub fn segment_blocked(&self, a: Vec3, b: Vec3, mask: u8) -> bool {
        let mut blocked = false;
        self.walk_segment(a, b, |i| {
            let o = &self.list[i];
            blocked = o.active && o.mask & mask != 0 && o.segment(a, b).is_some();
            blocked
        });
        blocked
    }

    /// Resolve a vertical capsule against movement-blocking obstacles.
    pub fn push_out(&self, mut p: Vec3, radius: f32, height: f32) -> Vec3 {
        for _ in 0..2 {
            let mut total = Vec2::ZERO;
            self.for_each_near(Vec2::new(p.x, p.z), radius + 1.0, |_, o| {
                if o.mask & MOVE != 0
                    && let Some(push) = o.push(p + Vec3::new(total.x, 0.0, total.y), radius, height)
                {
                    total += push;
                }
            });
            if total == Vec2::ZERO {
                break;
            }
            p.x += total.x;
            p.z += total.y;
        }
        p
    }

    /// First movement-blocking obstacle overlapping a vertical capsule.
    pub fn overlap(&self, p: Vec3, radius: f32, height: f32) -> Option<usize> {
        let mut found = None;
        self.for_each_near(Vec2::new(p.x, p.z), radius + 1.0, |i, o| {
            if found.is_none() && o.mask & MOVE != 0 && o.push(p, radius, height).is_some() {
                found = Some(i);
            }
        });
        found
    }
}

/// Line of sight between two points, blocked by ground and sight-blocking props.
pub fn line_of_sight(terrain: &Terrain, obstacles: &Obstacles, a: Vec3, b: Vec3) -> bool {
    terrain.raycast(a, b).is_none() && !obstacles.segment_blocked(a, b, SIGHT)
}

#[derive(Component)]
pub struct Tree {
    pub obstacle: usize,
    pub fall: Option<(Vec3, f32)>,
}

/// A tree was knocked over (replicated to clients).
#[derive(Message, Clone, Copy)]
pub struct TreeFell {
    pub obstacle: u32,
    pub dir: Vec2,
}

/// Building footprints and tree positions, used to draw the map.
#[derive(Resource, Default)]
pub struct MapFeatures {
    pub buildings: Vec<Footprint>,
    pub trees: Vec<Vec2>,
}

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Obstacles>()
            .init_resource::<MapFeatures>()
            .add_message::<TreeFell>()
            .add_systems(Startup, spawn_world)
            .add_systems(Update, (apply_tree_falls, animate_tree_falls).chain());
    }
}

fn apply_tree_falls(
    mut events: MessageReader<TreeFell>,
    mut obstacles: ResMut<Obstacles>,
    mut trees: Query<&mut Tree>,
) {
    for ev in events.read() {
        let Some(o) = obstacles.list.get_mut(ev.obstacle as usize) else {
            continue;
        };
        if !o.active {
            continue;
        }
        o.active = false;
        let entity = o.entity;
        // The crown blocks sight too; it was registered right after the trunk.
        if let Some(crown) = obstacles.list.get_mut(ev.obstacle as usize + 1)
            && crown.tree
            && crown.entity == entity
        {
            crown.active = false;
        }
        if let Some(e) = entity
            && let Ok(mut tree) = trees.get_mut(e)
        {
            let dir = ev.dir.normalize_or(Vec2::X);
            // Rotating around this axis tips the top of the tree toward `dir`.
            let axis = Vec3::new(-dir.y, 0.0, dir.x);
            tree.fall = Some((axis, 0.0));
        }
    }
}

fn animate_tree_falls(time: Res<Time>, mut trees: Query<(&mut Tree, &mut Transform)>) {
    for (mut tree, mut tf) in &mut trees {
        if let Some((axis, angle)) = tree.fall.as_mut() {
            if *angle >= FRAC_PI_2 - 0.05 {
                continue;
            }
            let step = (0.3 + *angle * 2.5) * time.delta_secs();
            let new = (*angle + step).min(FRAC_PI_2 - 0.05);
            tf.rotation = Quat::from_axis_angle(*axis, new - *angle) * tf.rotation;
            *angle = new;
        }
    }
}

// ---------------------------------------------------------------------------
// Mesh helpers
// ---------------------------------------------------------------------------

pub fn colored(mut mesh: Mesh, color: Color) -> Mesh {
    let c = LinearRgba::from(color).to_f32_array();
    let n = mesh.count_vertices();
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![c; n]);
    mesh
}

pub fn merged(parts: Vec<Mesh>) -> Mesh {
    let mut it = parts.into_iter();
    let mut m = it.next().expect("at least one mesh");
    for p in it {
        m.merge(&p).expect("compatible meshes");
    }
    m
}

/// Unit triangular prism (roof / tent): base 1 wide on X, 1 high, ridge along Z, 1 long.
pub fn prism_mesh() -> Mesh {
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let _ = s;
    let n_r = Vec3::new(1.0, 0.5, 0.0).normalize().to_array();
    let n_l = Vec3::new(-1.0, 0.5, 0.0).normalize().to_array();
    let positions: Vec<[f32; 3]> = vec![
        // front cap
        [-0.5, 0.0, 0.5],
        [0.5, 0.0, 0.5],
        [0.0, 1.0, 0.5],
        // back cap
        [0.5, 0.0, -0.5],
        [-0.5, 0.0, -0.5],
        [0.0, 1.0, -0.5],
        // right slope
        [0.5, 0.0, 0.5],
        [0.5, 0.0, -0.5],
        [0.0, 1.0, -0.5],
        [0.0, 1.0, 0.5],
        // left slope
        [-0.5, 0.0, -0.5],
        [-0.5, 0.0, 0.5],
        [0.0, 1.0, 0.5],
        [0.0, 1.0, -0.5],
    ];
    let normals: Vec<[f32; 3]> = vec![
        [0.0, 0.0, 1.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
        [0.0, 0.0, -1.0],
        [0.0, 0.0, -1.0],
        n_r,
        n_r,
        n_r,
        n_r,
        n_l,
        n_l,
        n_l,
        n_l,
    ];
    let uvs = vec![[0.0, 0.0]; positions.len()];
    let indices = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 6, 8, 9, 10, 11, 12, 10, 12, 13];
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

fn ico(r: f32, sub: u32) -> Mesh {
    Sphere::new(r).mesh().ico(sub).expect("valid icosphere")
}

// ---------------------------------------------------------------------------
// World generation
// ---------------------------------------------------------------------------

struct Props {
    cube: Handle<Mesh>,
    prism: Handle<Mesh>,
    windows: Handle<StandardMaterial>,
    door: Handle<StandardMaterial>,
    walls: Vec<Handle<StandardMaterial>>,
    roofs: Vec<Handle<StandardMaterial>>,
    stone: Handle<StandardMaterial>,
    sandbag: Handle<StandardMaterial>,
    hesco: Handle<StandardMaterial>,
    tent: Handle<StandardMaterial>,
    concrete: Handle<StandardMaterial>,
    metal: Handle<StandardMaterial>,
    flag_blue: Handle<StandardMaterial>,
    white: Handle<StandardMaterial>,
}

fn mat(materials: &mut Assets<StandardMaterial>, c: Color, rough: f32) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: c,
        perceptual_roughness: rough,
        ..default()
    })
}

#[allow(clippy::too_many_arguments)]
fn spawn_box(
    commands: &mut Commands,
    obstacles: &mut Obstacles,
    props: &Props,
    material: &Handle<StandardMaterial>,
    center: Vec3,
    size: Vec3,
    yaw: f32,
    mask: u8,
) {
    commands.spawn((
        Mesh3d(props.cube.clone()),
        MeshMaterial3d(material.clone()),
        Transform {
            translation: center,
            rotation: Quat::from_rotation_y(yaw),
            scale: size,
        },
    ));
    if mask != 0 {
        obstacles.add(Obstacle {
            pos: center,
            shape: Shape::Obb {
                half: size * 0.5,
                yaw,
            },
            mask,
            entity: None,
            active: true,
            tree: false,
        });
    }
}

fn ground_min_max(terrain: &Terrain, center: Vec2, half: Vec2, yaw: f32) -> (f32, f32) {
    let rot = Quat::from_rotation_y(yaw);
    let mut lo = terrain.height(center.x, center.y);
    let mut hi = lo;
    for (sx, sz) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let o = rot * Vec3::new(half.x * sx, 0.0, half.y * sz);
        let h = terrain.height(center.x + o.x, center.y + o.z);
        lo = lo.min(h);
        hi = hi.max(h);
    }
    (lo, hi)
}

#[allow(clippy::too_many_arguments)]
fn spawn_house(
    commands: &mut Commands,
    obstacles: &mut Obstacles,
    features: &mut MapFeatures,
    terrain: &Terrain,
    props: &Props,
    rng: &mut fastrand::Rng,
    center: Vec2,
    w: f32,
    d: f32,
    floors: u32,
    yaw: f32,
) {
    let (lo, hi) = ground_min_max(terrain, center, Vec2::new(w, d) * 0.5, yaw);
    let base = lo - 0.5;
    let floor0 = hi + 0.1;
    let top = floor0 + floors as f32 * 3.1 + 0.3;
    let wall_h = top - base;
    let rot = Quat::from_rotation_y(yaw);
    let wall_mat = &props.walls[rng.usize(..props.walls.len())];
    let c3 = Vec3::new(center.x, base + wall_h * 0.5, center.y);
    spawn_box(
        commands,
        obstacles,
        props,
        wall_mat,
        c3,
        Vec3::new(w, wall_h, d),
        yaw,
        SOLID,
    );
    let pitched = rng.f32() < 0.72;
    if pitched {
        let roof_h = (w * 0.38).min(3.5);
        let roof_mat = &props.roofs[rng.usize(..props.roofs.len())];
        commands.spawn((
            Mesh3d(props.prism.clone()),
            MeshMaterial3d(roof_mat.clone()),
            Transform {
                translation: Vec3::new(center.x, top, center.y),
                rotation: rot,
                scale: Vec3::new(w + 0.7, roof_h, d + 0.7),
            },
        ));
        obstacles.add(Obstacle {
            pos: Vec3::new(center.x, top + roof_h * 0.35, center.y),
            shape: Shape::Obb {
                half: Vec3::new(w * 0.35, roof_h * 0.35, d * 0.5),
                yaw,
            },
            mask: BULLET | SIGHT,
            entity: None,
            active: true,
            tree: false,
        });
    } else {
        // Flat roof with a small parapet.
        spawn_box(
            commands,
            obstacles,
            props,
            &props.concrete,
            Vec3::new(center.x, top + 0.15, center.y),
            Vec3::new(w + 0.3, 0.3, d + 0.3),
            yaw,
            0,
        );
    }
    // Windows and a door on each facade.
    for side in 0..4 {
        let (len, out, axis_x) = match side {
            0 => (w, d * 0.5, true),
            1 => (w, -d * 0.5, true),
            2 => (d, w * 0.5, false),
            _ => (d, -w * 0.5, false),
        };
        let count = ((len - 1.5) / 3.0).floor().max(1.0) as i32;
        for floor in 0..floors {
            let y = floor0 + floor as f32 * 3.1 + 1.55;
            for k in 0..count {
                let along = (k as f32 + 0.5) / count as f32 * len - len * 0.5;
                let is_door = side == 0 && floor == 0 && k == count / 2;
                let (local, size) = if axis_x {
                    (
                        Vec3::new(along, 0.0, out + out.signum() * 0.03),
                        Vec3::new(if is_door { 1.1 } else { 1.0 }, 1.0, 0.08),
                    )
                } else {
                    (
                        Vec3::new(out + out.signum() * 0.03, 0.0, along),
                        Vec3::new(0.08, 1.0, 1.0),
                    )
                };
                let (h, yy) = if is_door { (2.1, floor0 + 1.05) } else { (1.2, y) };
                let p = rot * local;
                commands.spawn((
                    Mesh3d(props.cube.clone()),
                    MeshMaterial3d(if is_door {
                        props.door.clone()
                    } else {
                        props.windows.clone()
                    }),
                    Transform {
                        translation: Vec3::new(center.x + p.x, yy, center.y + p.z),
                        rotation: rot,
                        scale: Vec3::new(size.x, h, size.z),
                    },
                    NotShadowCaster,
                ));
            }
        }
    }
    // Garden wall around some houses.
    if rng.f32() < 0.35 {
        let m = 3.5;
        let (hw, hd) = (w * 0.5 + m, d * 0.5 + m);
        let segments = [
            (Vec3::new(0.0, 0.0, -hd), Vec3::new(w + 2.0 * m, 1.3, 0.35)),
            (Vec3::new(-hw, 0.0, 0.0), Vec3::new(0.35, 1.3, d + 2.0 * m)),
            (Vec3::new(hw, 0.0, 0.0), Vec3::new(0.35, 1.3, d + 2.0 * m)),
        ];
        for (local, size) in segments {
            let p = rot * local;
            let wp = Vec2::new(center.x + p.x, center.y + p.z);
            let g = terrain.height(wp.x, wp.y);
            spawn_box(
                commands,
                obstacles,
                props,
                &props.stone,
                Vec3::new(wp.x, g + 0.45, wp.y),
                size + Vec3::new(0.0, 0.5, 0.0),
                yaw,
                MOVE | BULLET,
            );
        }
    }
    features.buildings.push(Footprint {
        center,
        half: Vec2::new(w, d) * 0.5,
        yaw,
    });
}

fn nearest_road_dir(p: Vec2) -> (f32, Vec2) {
    let mut best = (f32::MAX, Vec2::X);
    for &(a, b) in road_segments() {
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
        let d = p.distance(a + ab * t);
        if d < best.0 {
            best = (d, ab.normalize_or(Vec2::X));
        }
    }
    best
}

#[allow(clippy::too_many_arguments)]
fn spawn_sandbag_ring(
    commands: &mut Commands,
    obstacles: &mut Obstacles,
    terrain: &Terrain,
    props: &Props,
    center: Vec2,
    radius: f32,
    facing: f32,
    arc: f32,
) {
    let n = ((radius * arc) / 2.6).ceil().max(2.0) as i32;
    for i in 0..n {
        let a = facing - arc * 0.5 + arc * (i as f32 + 0.5) / n as f32;
        let p = center + Vec2::new(-a.sin(), -a.cos()) * radius;
        let g = terrain.height(p.x, p.y);
        spawn_box(
            commands,
            obstacles,
            props,
            &props.sandbag,
            Vec3::new(p.x, g + 0.35, p.y),
            Vec3::new(2.8, 1.1, 0.8),
            a,
            MOVE | BULLET,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_tower(
    commands: &mut Commands,
    obstacles: &mut Obstacles,
    terrain: &Terrain,
    props: &Props,
    p: Vec2,
) {
    let g = terrain.height(p.x, p.y);
    for (sx, sz) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        spawn_box(
            commands,
            obstacles,
            props,
            &props.metal,
            Vec3::new(p.x + sx * 1.3, g + 3.0, p.y + sz * 1.3),
            Vec3::new(0.25, 6.0, 0.25),
            0.0,
            BULLET,
        );
    }
    spawn_box(
        commands,
        obstacles,
        props,
        &props.sandbag,
        Vec3::new(p.x, g + 6.5, p.y),
        Vec3::new(3.2, 1.0, 3.2),
        0.0,
        BULLET | SIGHT,
    );
    spawn_box(
        commands,
        obstacles,
        props,
        &props.metal,
        Vec3::new(p.x, g + 8.6, p.y),
        Vec3::new(3.4, 0.15, 3.4),
        0.0,
        0,
    );
}

#[allow(clippy::too_many_arguments)]
fn spawn_town(
    commands: &mut Commands,
    obstacles: &mut Obstacles,
    features: &mut MapFeatures,
    terrain: &Terrain,
    props: &Props,
    rng: &mut fastrand::Rng,
    loc: &Location,
) {
    let target = match loc.kind {
        LocKind::Town => (loc.radius * loc.radius / 230.0) as usize,
        LocKind::Village => (loc.radius * loc.radius / 330.0) as usize,
        _ => 3,
    };
    let mut placed: Vec<(Vec2, f32)> = Vec::new();
    let mut attempts = 0;
    while placed.len() < target && attempts < target * 40 {
        attempts += 1;
        let p = loc.pos + rand_in_circle_rng(rng) * loc.radius * 0.95;
        if p.distance(loc.pos) < 18.0 {
            continue;
        }
        let w = 6.0 + rng.f32() * 5.0;
        let d = 7.0 + rng.f32() * 6.0;
        let r = Vec2::new(w, d).length() * 0.5;
        let (road_d, road_dir) = nearest_road_dir(p);
        if road_d < r + 5.0 {
            continue;
        }
        if placed.iter().any(|&(q, qr)| q.distance(p) < r + qr + 4.0) {
            continue;
        }
        let normal = terrain.normal(p.x, p.y);
        if normal.y < 0.9 || terrain.height(p.x, p.y) < 2.0 {
            continue;
        }
        let mut yaw = f32::atan2(-road_dir.x, -road_dir.y);
        if road_d > 45.0 {
            yaw += (rng.u32(0..4) as f32) * FRAC_PI_2 + rng.f32() * 0.3;
        }
        let floors = if rng.f32() < 0.3 { 2 } else { 1 };
        spawn_house(
            commands, obstacles, features, terrain, props, rng, p, w, d, floors, yaw,
        );
        placed.push((p, r));
    }
    // Town centre landmark.
    if loc.kind == LocKind::Town {
        let p = loc.pos + Vec2::new(0.0, -9.0);
        let g = terrain.height(p.x, p.y);
        spawn_box(
            commands,
            obstacles,
            props,
            &props.walls[0],
            Vec3::new(p.x, g + 3.0, p.y + 6.0),
            Vec3::new(8.0, 7.0, 14.0),
            0.0,
            SOLID,
        );
        commands.spawn((
            Mesh3d(props.prism.clone()),
            MeshMaterial3d(props.roofs[1].clone()),
            Transform {
                translation: Vec3::new(p.x, g + 6.5, p.y + 6.0),
                scale: Vec3::new(8.6, 3.5, 14.6),
                ..default()
            },
        ));
        spawn_box(
            commands,
            obstacles,
            props,
            &props.walls[0],
            Vec3::new(p.x, g + 8.0, p.y - 3.0),
            Vec3::new(4.5, 17.0, 4.5),
            0.0,
            SOLID,
        );
        commands.spawn((
            Mesh3d(props.prism.clone()),
            MeshMaterial3d(props.roofs[2].clone()),
            Transform {
                translation: Vec3::new(p.x, g + 16.5, p.y - 3.0),
                scale: Vec3::new(4.8, 5.0, 4.8),
                ..default()
            },
        ));
        features.buildings.push(Footprint {
            center: p + Vec2::new(0.0, 2.0),
            half: Vec2::new(4.0, 9.5),
            yaw: 0.0,
        });
    }
    // Defensive positions facing outward along each road into town.
    for &(a, b) in road_segments() {
        for p in [a, b] {
            let d = p.distance(loc.pos);
            if (loc.radius * 0.6..loc.radius * 1.1).contains(&d) && rng.f32() < 0.6 {
                let out = (p - loc.pos).normalize();
                let facing = f32::atan2(-out.x, -out.y);
                let c = p + Vec2::new(out.y, -out.x) * 9.0;
                spawn_sandbag_ring(commands, obstacles, terrain, props, c, 3.0, facing, 2.4);
            }
        }
    }
    if loc.kind == LocKind::Outpost {
        for k in 0..4 {
            let a = k as f32 * FRAC_PI_2 + 0.4;
            let c = loc.pos + Vec2::new(-a.sin(), -a.cos()) * 22.0;
            spawn_sandbag_ring(commands, obstacles, terrain, props, c, 3.2, a, 2.6);
        }
        spawn_tower(commands, obstacles, terrain, props, loc.pos + Vec2::new(8.0, 8.0));
    }
}

fn rand_in_circle_rng(rng: &mut fastrand::Rng) -> Vec2 {
    let a = rng.f32() * std::f32::consts::TAU;
    let r = rng.f32().sqrt();
    Vec2::new(a.cos() * r, a.sin() * r)
}

fn spawn_base(
    commands: &mut Commands,
    obstacles: &mut Obstacles,
    features: &mut MapFeatures,
    terrain: &Terrain,
    props: &Props,
) {
    let c = LOCATIONS[BASE].pos;
    let half = 62.0;
    // HESCO perimeter with a gate on the east side (toward the island).
    for side in 0..4 {
        let n = (half * 2.0 / 1.5) as i32;
        for i in 0..n {
            let t = -half + (i as f32 + 0.5) * 1.5;
            let (x, z) = match side {
                0 => (t, -half),
                1 => (t, half),
                2 => (-half, t),
                _ => (half, t),
            };
            if side == 3 && t.abs() < 7.0 {
                continue;
            }
            let p = c + Vec2::new(x, z);
            let g = terrain.height(p.x, p.y);
            spawn_box(
                commands,
                obstacles,
                props,
                &props.hesco,
                Vec3::new(p.x, g + 0.6, p.y),
                Vec3::new(1.5, 2.2, 1.5),
                0.0,
                SOLID,
            );
        }
    }
    // Tents.
    for (i, off) in [
        Vec2::new(-40.0, -40.0),
        Vec2::new(-28.0, -40.0),
        Vec2::new(-16.0, -40.0),
        Vec2::new(-40.0, -22.0),
    ]
    .iter()
    .enumerate()
    {
        let p = c + *off;
        let g = terrain.height(p.x, p.y);
        commands.spawn((
            Mesh3d(props.prism.clone()),
            MeshMaterial3d(props.tent.clone()),
            Transform {
                translation: Vec3::new(p.x, g - 0.1, p.y),
                rotation: Quat::from_rotation_y(if i == 3 { FRAC_PI_2 } else { 0.0 }),
                scale: Vec3::new(5.5, 3.2, 9.0),
            },
        ));
        obstacles.add(Obstacle {
            pos: Vec3::new(p.x, g + 1.2, p.y),
            shape: Shape::Obb {
                half: Vec3::new(2.4, 1.4, 4.5),
                yaw: if i == 3 { FRAC_PI_2 } else { 0.0 },
            },
            mask: SOLID,
            entity: None,
            active: true,
            tree: false,
        });
        features.buildings.push(Footprint {
            center: p,
            half: Vec2::new(2.75, 4.5),
            yaw: if i == 3 { FRAC_PI_2 } else { 0.0 },
        });
    }
    // Containers.
    for k in 0..5 {
        let p = c + Vec2::new(30.0, -45.0 + k as f32 * 3.0);
        let g = terrain.height(p.x, p.y);
        spawn_box(
            commands,
            obstacles,
            props,
            &props.metal,
            Vec3::new(p.x, g + 1.3, p.y),
            Vec3::new(6.0, 2.6, 2.4),
            0.0,
            SOLID,
        );
    }
    // Helipad.
    let pad = c + Vec2::new(25.0, 30.0);
    let g = terrain.height(pad.x, pad.y);
    commands.spawn((
        Mesh3d(props.cube.clone()),
        MeshMaterial3d(props.concrete.clone()),
        Transform::from_xyz(pad.x, g + 0.02, pad.y).with_scale(Vec3::new(16.0, 0.2, 16.0)),
    ));
    for (off, size) in [
        (Vec3::new(-2.0, 0.0, 0.0), Vec3::new(0.8, 0.1, 6.0)),
        (Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.8, 0.1, 6.0)),
        (Vec3::ZERO, Vec3::new(4.0, 0.1, 0.8)),
    ] {
        commands.spawn((
            Mesh3d(props.cube.clone()),
            MeshMaterial3d(props.white.clone()),
            Transform::from_xyz(pad.x + off.x, g + 0.1, pad.y + off.z).with_scale(size),
            NotShadowCaster,
        ));
    }
    // Flag.
    let fp = c + Vec2::new(-5.0, 5.0);
    let g = terrain.height(fp.x, fp.y);
    spawn_box(
        commands,
        obstacles,
        props,
        &props.metal,
        Vec3::new(fp.x, g + 5.0, fp.y),
        Vec3::new(0.12, 10.0, 0.12),
        0.0,
        0,
    );
    commands.spawn((
        Mesh3d(props.cube.clone()),
        MeshMaterial3d(props.flag_blue.clone()),
        Transform::from_xyz(fp.x + 1.1, g + 9.2, fp.y).with_scale(Vec3::new(2.2, 1.3, 0.04)),
    ));
    spawn_tower(commands, obstacles, terrain, props, c + Vec2::new(half - 6.0, -half + 6.0));
    spawn_tower(commands, obstacles, terrain, props, c + Vec2::new(-half + 6.0, half - 6.0));
}

fn spawn_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    terrain: Res<Terrain>,
    mut obstacles: ResMut<Obstacles>,
    mut features: ResMut<MapFeatures>,
) {
    // Sun and sky.
    commands.spawn((
        DirectionalLight {
            illuminance: 11_000.0,
            shadow_maps_enabled: true,
            color: Color::srgb(1.0, 0.96, 0.88),
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.9, -0.8, 0.0)),
        CascadeShadowConfigBuilder {
            num_cascades: 3,
            first_cascade_far_bound: 25.0,
            maximum_distance: 260.0,
            ..default()
        }
        .build(),
        bevy::camera::visibility::RenderLayers::from_layers(&[0, 1]),
    ));

    // Terrain chunks.
    let ground = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.95,
        reflectance: 0.2,
        ..default()
    });
    for cz in 0..CHUNKS {
        for cx in 0..CHUNKS {
            commands.spawn((
                Mesh3d(meshes.add(terrain.chunk_mesh(cx, cz))),
                MeshMaterial3d(ground.clone()),
                Transform::IDENTITY,
            ));
        }
    }
    // Sea.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::new(Vec3::Y, Vec2::splat(9000.0)))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(0.12, 0.28, 0.38, 0.88),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.08,
            reflectance: 0.6,
            ..default()
        })),
        Transform::from_xyz(0.0, WATER_LEVEL, 0.0),
        NotShadowCaster,
    ));

    let props = Props {
        cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        prism: meshes.add(prism_mesh()),
        windows: materials.add(StandardMaterial {
            base_color: Color::srgb(0.08, 0.1, 0.12),
            perceptual_roughness: 0.15,
            metallic: 0.3,
            ..default()
        }),
        door: mat(&mut materials, Color::srgb(0.3, 0.2, 0.12), 0.8),
        walls: [
            (0.86, 0.84, 0.78),
            (0.8, 0.74, 0.6),
            (0.85, 0.8, 0.62),
            (0.7, 0.7, 0.68),
            (0.82, 0.72, 0.68),
            (0.62, 0.58, 0.52),
        ]
        .iter()
        .map(|&(r, g, b)| mat(&mut materials, Color::srgb(r, g, b), 0.9))
        .collect(),
        roofs: [(0.55, 0.22, 0.15), (0.3, 0.3, 0.32), (0.4, 0.28, 0.2), (0.28, 0.36, 0.3)]
            .iter()
            .map(|&(r, g, b)| mat(&mut materials, Color::srgb(r, g, b), 0.8))
            .collect(),
        stone: mat(&mut materials, Color::srgb(0.55, 0.53, 0.5), 0.95),
        sandbag: mat(&mut materials, Color::srgb(0.6, 0.55, 0.4), 1.0),
        hesco: mat(&mut materials, Color::srgb(0.66, 0.6, 0.45), 1.0),
        tent: mat(&mut materials, Color::srgb(0.32, 0.36, 0.25), 1.0),
        concrete: mat(&mut materials, Color::srgb(0.45, 0.45, 0.45), 0.9),
        metal: mat(&mut materials, Color::srgb(0.35, 0.38, 0.33), 0.6),
        flag_blue: mat(&mut materials, Color::srgb(0.1, 0.25, 0.7), 0.9),
        white: mat(&mut materials, Color::srgb(0.95, 0.95, 0.95), 0.9),
    };

    let mut rng = fastrand::Rng::with_seed(0x5EED_1234);
    for (i, loc) in LOCATIONS.iter().enumerate() {
        if i == BASE {
            spawn_base(&mut commands, &mut obstacles, &mut features, &terrain, &props);
        } else {
            spawn_town(
                &mut commands,
                &mut obstacles,
                &mut features,
                &terrain,
                &props,
                &mut rng,
                loc,
            );
        }
    }

    // Vegetation: vertex-coloured merged meshes so each plant is a single entity.
    let bark = Color::srgb(0.3, 0.22, 0.15);
    let pine = Color::srgb(0.12, 0.24, 0.12);
    let leaf = Color::srgb(0.24, 0.36, 0.15);
    let conifer = meshes.add(merged(vec![
        colored(
            Cylinder::new(0.22, 3.0).mesh().resolution(6).build(),
            bark,
        )
        .translated_by(Vec3::Y * 1.5),
        colored(Cone::new(2.3, 5.0).mesh().resolution(8).build(), pine)
            .translated_by(Vec3::Y * 4.6),
        colored(Cone::new(1.7, 4.0).mesh().resolution(8).build(), pine)
            .translated_by(Vec3::Y * 7.2),
    ]));
    let broadleaf = meshes.add(merged(vec![
        colored(
            Cylinder::new(0.28, 4.0).mesh().resolution(6).build(),
            bark,
        )
        .translated_by(Vec3::Y * 2.0),
        colored(ico(2.7, 1), leaf).translated_by(Vec3::new(0.0, 5.4, 0.0)),
        colored(ico(2.0, 1), leaf).translated_by(Vec3::new(1.3, 4.6, 0.6)),
        colored(ico(1.9, 1), leaf).translated_by(Vec3::new(-1.1, 4.9, -0.8)),
    ]));
    let bush = meshes.add(
        colored(ico(1.2, 1), Color::srgb(0.22, 0.32, 0.14))
            .scaled_by(Vec3::new(1.3, 0.8, 1.3))
            .translated_by(Vec3::Y * 0.6),
    );
    let rock = meshes.add(colored(ico(1.0, 1), Color::srgb(0.48, 0.47, 0.45)));
    let veg_mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.95,
        ..default()
    });

    let spacing = 11.0;
    let steps = (MAP_HALF * 2.0 / spacing) as i32;
    for gz in 0..steps {
        for gx in 0..steps {
            let x = -MAP_HALF + (gx as f32 + rng.f32()) * spacing;
            let z = -MAP_HALF + (gz as f32 + rng.f32()) * spacing;
            let forest = fbm(x * 0.0028, z * 0.0028, 4, 21);
            let roll = rng.f32();
            let kind_roll = rng.f32();
            let scale = 0.75 + rng.f32() * 0.55;
            let yaw = rng.f32() * PI * 2.0;
            let chance = if forest > 0.56 {
                ((forest - 0.56) * 5.0).min(0.85)
            } else {
                0.008
            };
            let bush_chance = if forest > 0.5 { 0.06 } else { 0.012 };
            let rock_chance = 0.004;
            if roll > chance + bush_chance + rock_chance {
                continue;
            }
            let h = terrain.height(x, z);
            let p = Vec2::new(x, z);
            if h < 1.5 || settlement_factor(p) > 0.02 || road_distance(p) < 7.0 {
                continue;
            }
            if p.distance(LOCATIONS[BASE].pos) < 110.0 {
                continue;
            }
            let normal = terrain.normal(x, z);
            if roll < chance {
                if normal.y < 0.8 {
                    continue;
                }
                let is_conifer = kind_roll < 0.55 + (h / 200.0);
                let e = commands
                    .spawn((
                        Mesh3d(if is_conifer {
                            conifer.clone()
                        } else {
                            broadleaf.clone()
                        }),
                        MeshMaterial3d(veg_mat.clone()),
                        Transform {
                            translation: Vec3::new(x, h - 0.2, z),
                            rotation: Quat::from_rotation_y(yaw),
                            scale: Vec3::splat(scale),
                        },
                        VisibilityRange::abrupt(0.0, 1500.0),
                    ))
                    .id();
                let trunk = obstacles.add(Obstacle {
                    pos: Vec3::new(x, h - 0.2, z),
                    shape: Shape::Cyl {
                        r: 0.3 * scale,
                        h: if is_conifer { 9.0 } else { 7.0 } * scale,
                    },
                    mask: MOVE | BULLET,
                    entity: Some(e),
                    active: true,
                    tree: true,
                });
                obstacles.add(Obstacle {
                    pos: Vec3::new(x, h + if is_conifer { 5.0 } else { 5.0 } * scale, z),
                    shape: Shape::Sphere {
                        r: if is_conifer { 2.3 } else { 3.0 } * scale,
                    },
                    mask: SIGHT,
                    entity: Some(e),
                    active: true,
                    tree: true,
                });
                commands.entity(e).insert(Tree {
                    obstacle: trunk,
                    fall: None,
                });
                features.trees.push(p);
            } else if roll < chance + bush_chance {
                commands.spawn((
                    Mesh3d(bush.clone()),
                    MeshMaterial3d(veg_mat.clone()),
                    Transform {
                        translation: Vec3::new(x, h - 0.1, z),
                        rotation: Quat::from_rotation_y(yaw),
                        scale: Vec3::splat(scale),
                    },
                    VisibilityRange::abrupt(0.0, 600.0),
                ));
                obstacles.add(Obstacle {
                    pos: Vec3::new(x, h + 0.6 * scale, z),
                    shape: Shape::Sphere { r: 1.4 * scale },
                    mask: SIGHT,
                    entity: None,
                    active: true,
                    tree: false,
                });
            } else {
                let s = Vec3::new(1.0 + rng.f32() * 1.5, 0.6 + rng.f32(), 1.0 + rng.f32() * 1.5);
                commands.spawn((
                    Mesh3d(rock.clone()),
                    MeshMaterial3d(veg_mat.clone()),
                    Transform {
                        translation: Vec3::new(x, h, z),
                        rotation: Quat::from_rotation_y(yaw),
                        scale: s,
                    },
                    VisibilityRange::abrupt(0.0, 900.0),
                ));
                obstacles.add(Obstacle {
                    pos: Vec3::new(x, h, z),
                    shape: Shape::Sphere {
                        r: s.x.min(s.z).max(s.y) * 0.95,
                    },
                    mask: SOLID,
                    entity: None,
                    active: true,
                    tree: false,
                });
            }
        }
    }
    info!(
        "World generated: {} obstacles, {} trees, {} buildings",
        obstacles.list.len(),
        features.trees.len(),
        features.buildings.len()
    );
}
