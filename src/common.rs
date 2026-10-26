//! Types and helpers shared by every module.

use std::f32::consts::{PI, TAU};

use bevy::prelude::*;

pub const GRAVITY: f32 = 9.81;
pub const SKY_COLOR: Color = Color::srgb(0.64, 0.74, 0.86);

/// Ordering of the per-frame simulation.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum Phase {
    Input,
    Ai,
    Movement,
    Combat,
    Damage,
    Camera,
    Hud,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Faction {
    Blufor,
    Opfor,
}

impl Faction {
    pub fn enemy(self) -> Self {
        match self {
            Faction::Blufor => Faction::Opfor,
            Faction::Opfor => Faction::Blufor,
        }
    }

    pub fn map_color(self) -> Color {
        match self {
            Faction::Blufor => Color::srgb(0.15, 0.4, 0.95),
            Faction::Opfor => Color::srgb(0.9, 0.12, 0.1),
        }
    }
}

/// The soldier currently controlled by the human.
#[derive(Component)]
pub struct Player;

/// Everything that belongs to the current mission run and is despawned on restart.
#[derive(Component)]
pub struct MissionEntity;

#[derive(Component)]
pub struct Health {
    pub hp: f32,
    pub max: f32,
    /// 0 = unarmoured, 1 = light vehicle, 2 = medium, 3 = heavy.
    pub armor: u8,
    pub dead: bool,
}

impl Health {
    pub fn new(hp: f32, armor: u8) -> Self {
        Self {
            hp,
            max: hp,
            armor,
            dead: false,
        }
    }
}

#[derive(Message, Clone)]
pub struct Damage {
    pub target: Entity,
    pub amount: f32,
    /// Armour penetration class of the projectile.
    pub ap: u8,
    pub attacker: Option<Entity>,
    pub headshot: bool,
}

#[derive(Message, Clone)]
pub struct Killed {
    pub victim: Entity,
    pub attacker: Option<Entity>,
}

/// A line in the radio / kill feed.
#[derive(Message, Clone)]
pub struct Radio {
    pub text: String,
    pub color: Color,
}

impl Radio {
    pub fn new(text: impl Into<String>, color: Color) -> Self {
        Self {
            text: text.into(),
            color,
        }
    }
}

pub const RADIO_SIDE: Color = Color::srgb(0.55, 0.8, 1.0);
pub const RADIO_GROUP: Color = Color::srgb(0.75, 1.0, 0.6);
pub const RADIO_SYSTEM: Color = Color::srgb(1.0, 0.85, 0.4);
pub const RADIO_KILL: Color = Color::srgb(1.0, 0.5, 0.45);

/// Short-lived feedback the HUD reacts to.
#[derive(Resource, Default)]
pub struct PlayerFeedback {
    pub hurt: f32,
    pub hitmarker: f32,
}

/// Damage multiplier for a projectile of penetration `ap` against `armor`.
pub fn armor_factor(ap: u8, armor: u8) -> f32 {
    if ap >= armor {
        1.0
    } else if ap + 1 == armor {
        0.12
    } else {
        0.0
    }
}

/// Unit direction for a yaw (around +Y, 0 = looking down -Z) and pitch (up positive).
pub fn dir_from_angles(yaw: f32, pitch: f32) -> Vec3 {
    Vec3::new(
        -yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    )
}

pub fn angles_of(dir: Vec3) -> (f32, f32) {
    let d = dir.normalize_or(Vec3::NEG_Z);
    (f32::atan2(-d.x, -d.z), d.y.clamp(-1.0, 1.0).asin())
}

pub fn forward_of_yaw(yaw: f32) -> Vec3 {
    Vec3::new(-yaw.sin(), 0.0, -yaw.cos())
}

pub fn right_of_yaw(yaw: f32) -> Vec3 {
    Vec3::new(yaw.cos(), 0.0, -yaw.sin())
}

/// Signed shortest rotation from `from` to `to`, in (-PI, PI].
pub fn angle_diff(from: f32, to: f32) -> f32 {
    let mut d = (to - from) % TAU;
    if d > PI {
        d -= TAU;
    } else if d < -PI {
        d += TAU;
    }
    d
}

/// Turn `current` toward `target` by at most `max_step` radians.
pub fn turn_toward(current: f32, target: f32, max_step: f32) -> f32 {
    let d = angle_diff(current, target);
    current + d.clamp(-max_step, max_step)
}

pub fn rand_range(lo: f32, hi: f32) -> f32 {
    lo + fastrand::f32() * (hi - lo)
}

pub fn rand_in_circle() -> Vec2 {
    let a = fastrand::f32() * TAU;
    let r = fastrand::f32().sqrt();
    Vec2::new(a.cos() * r, a.sin() * r)
}

pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Closest points between segments p1-q1 and p2-q2. Returns (s, t, squared distance).
pub fn closest_segments(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> (f32, f32, f32) {
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.length_squared();
    let e = d2.length_squared();
    let f = d2.dot(r);
    let (s, t);
    if a <= 1e-8 && e <= 1e-8 {
        return (0.0, 0.0, r.length_squared());
    }
    if a <= 1e-8 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= 1e-8 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s0 = if denom.abs() > 1e-8 {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t0 = 0.0;
                s0 = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t0 = 1.0;
                s0 = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = s0;
            t = t0;
        }
    }
    let c1 = p1 + d1 * s;
    let c2 = p2 + d2 * t;
    (s, t, c1.distance_squared(c2))
}

/// Entry parameter of segment a->b into a sphere, if it starts outside and enters it.
pub fn segment_sphere(a: Vec3, b: Vec3, center: Vec3, r: f32) -> Option<f32> {
    let d = b - a;
    let m = a - center;
    let c = m.length_squared() - r * r;
    if c < 0.0 {
        return None;
    }
    let aa = d.length_squared();
    if aa < 1e-10 {
        return None;
    }
    let bb = m.dot(d);
    let disc = bb * bb - aa * c;
    if disc < 0.0 {
        return None;
    }
    let t = (-bb - disc.sqrt()) / aa;
    (0.0..=1.0).contains(&t).then_some(t)
}

/// Entry parameter of segment a->b into an oriented box given in the box's local space.
pub fn segment_box_local(a: Vec3, b: Vec3, half: Vec3) -> Option<f32> {
    if a.x.abs() < half.x && a.y.abs() < half.y && a.z.abs() < half.z {
        return None;
    }
    let d = b - a;
    let mut tmin = 0.0f32;
    let mut tmax = 1.0f32;
    for i in 0..3 {
        let (ai, di, hi) = (a[i], d[i], half[i]);
        if di.abs() < 1e-8 {
            if ai.abs() > hi {
                return None;
            }
        } else {
            let mut t1 = (-hi - ai) / di;
            let mut t2 = (hi - ai) / di;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            tmin = tmin.max(t1);
            tmax = tmax.min(t2);
            if tmin > tmax {
                return None;
            }
        }
    }
    Some(tmin)
}

/// Military grid reference (100 m squares) for a world position.
pub fn grid_ref(pos: Vec3) -> String {
    let x = ((pos.x + 1024.0) / 10.0).floor().clamp(0.0, 999.0) as i32;
    let y = ((1024.0 - pos.z) / 10.0).floor().clamp(0.0, 999.0) as i32;
    format!("{x:03} {y:03}")
}
