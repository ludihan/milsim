//! Vehicles: specs, models, driving and flight physics, turrets, seats, wrecks and AI drivers.

use std::f32::consts::FRAC_PI_2;

use bevy::{light::NotShadowCaster, prelude::*};

use crate::ai::{Targets, ballistic_aim};
use crate::audio::{PlaySound, SoundKind};
use crate::combat::{
    ExplosionFx, FxAssets, Shooter, Weapon, WeaponKind, apply_spread, spawn_puff,
};
use crate::common::*;
use crate::net::is_authority;
use crate::soldier::Soldier;
use crate::terrain::{MAP_HALF, Terrain, WATER_LEVEL};
use crate::world::{MOVE, Obstacles, Shape, TreeFell, line_of_sight};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum VehicleKind {
    Jeep,
    Ifv,
    Heli,
}

impl VehicleKind {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => VehicleKind::Ifv,
            2 => VehicleKind::Heli,
            _ => VehicleKind::Jeep,
        }
    }
}

pub struct VehicleSpec {
    /// Collision half extents; the box sits on the vehicle origin.
    pub half: Vec3,
    pub max_speed: f32,
    pub accel: f32,
    pub turn_rate: f32,
    pub hp: f32,
    pub armor: u8,
    pub seats: usize,
    pub weapons: &'static [(WeaponKind, u32)],
    pub turret_pos: Vec3,
    pub barrel_len: f32,
    pub turret_speed: f32,
    pub pitch_limits: (f32, f32),
    pub cam_dist: f32,
    pub eye: Vec3,
    pub tracked: bool,
}

static JEEP: VehicleSpec = VehicleSpec {
    half: Vec3::new(1.1, 1.0, 2.45),
    max_speed: 24.0,
    accel: 6.0,
    turn_rate: 0.9,
    hp: 380.0,
    armor: 1,
    seats: 4,
    weapons: &[(WeaponKind::Hmg, 4)],
    turret_pos: Vec3::new(0.0, 2.25, 0.5),
    barrel_len: 1.5,
    turret_speed: 2.5,
    pitch_limits: (-0.3, 0.7),
    cam_dist: 9.0,
    eye: Vec3::new(0.0, 2.75, 1.1),
    tracked: false,
};

static IFV: VehicleSpec = VehicleSpec {
    half: Vec3::new(1.7, 1.25, 3.3),
    max_speed: 14.0,
    accel: 3.5,
    turn_rate: 0.75,
    hp: 1200.0,
    armor: 3,
    seats: 6,
    weapons: &[(WeaponKind::Cannon, 10), (WeaponKind::Coax, 4)],
    turret_pos: Vec3::new(0.0, 2.55, 0.3),
    barrel_len: 3.0,
    turret_speed: 0.9,
    pitch_limits: (-0.15, 0.45),
    cam_dist: 13.0,
    eye: Vec3::new(0.3, 3.45, 0.6),
    tracked: true,
};

static HELI: VehicleSpec = VehicleSpec {
    half: Vec3::new(1.15, 1.4, 3.4),
    max_speed: 70.0,
    accel: 0.0,
    turn_rate: 1.3,
    hp: 650.0,
    armor: 1,
    seats: 6,
    weapons: &[(WeaponKind::Minigun, 3), (WeaponKind::Rockets, 2)],
    turret_pos: Vec3::new(0.0, 0.7, -2.7),
    barrel_len: 0.9,
    turret_speed: 3.0,
    pitch_limits: (-1.3, 0.2),
    cam_dist: 17.0,
    eye: Vec3::new(0.0, 2.05, -2.0),
    tracked: false,
};

#[derive(Clone, Default, Debug)]
pub struct VehicleInput {
    pub throttle: f32,
    pub steer: f32,
    pub brake: bool,
    pub collective: f32,
    pub cyclic_pitch: f32,
    pub cyclic_roll: f32,
    pub pedal: f32,
    pub fire: bool,
    pub switch_weapon: bool,
    pub aim_point: Option<Vec3>,
}

#[derive(Component)]
pub struct Vehicle {
    pub kind: VehicleKind,
    pub paint: Faction,
    pub driver: Option<Entity>,
    pub passengers: Vec<Entity>,
    pub crew_faction: Option<Faction>,
    pub speed: f32,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub input: VehicleInput,
    pub weapons: Vec<Weapon>,
    pub active: usize,
    pub turret_yaw: f32,
    pub turret_pitch: f32,
    pub destroyed: bool,
    pub wrecked_visual: bool,
    pub rotor: f32,
    pub airborne: bool,
    pub smoke_timer: f32,
    pub turret: Entity,
    pub gun: Entity,
    pub rotors: Vec<(Entity, Vec3)>,
    pub parts: Vec<Entity>,
}

impl Vehicle {
    pub fn spec(&self) -> &'static VehicleSpec {
        match self.kind {
            VehicleKind::Jeep => &JEEP,
            VehicleKind::Ifv => &IFV,
            VehicleKind::Heli => &HELI,
        }
    }

    pub fn name(&self) -> &'static str {
        match (self.kind, self.paint) {
            (VehicleKind::Jeep, Faction::Blufor) => "HMMWV (M2)",
            (VehicleKind::Jeep, Faction::Opfor) => "UAZ-469 (DShK)",
            (VehicleKind::Ifv, Faction::Blufor) => "M2A3 Bradley",
            (VehicleKind::Ifv, Faction::Opfor) => "BMP-2",
            (VehicleKind::Heli, Faction::Blufor) => "UH-1Y Venom",
            (VehicleKind::Heli, Faction::Opfor) => "Mi-8MT",
        }
    }

    pub fn occupants(&self) -> impl Iterator<Item = Entity> + '_ {
        self.driver.iter().copied().chain(self.passengers.iter().copied())
    }

    pub fn has_seat(&self) -> bool {
        self.driver.is_none() || self.passengers.len() + 1 < self.spec().seats
    }

    pub fn remove_occupant(&mut self, e: Entity) {
        if self.driver == Some(e) {
            self.driver = None;
        }
        self.passengers.retain(|&p| p != e);
        if self.driver.is_none() && self.passengers.is_empty() {
            self.crew_faction = None;
        }
    }

    /// World-space barrel direction and muzzle position.
    pub fn barrel(&self, tf: &Transform) -> (Vec3, Vec3) {
        let spec = self.spec();
        let pivot = tf.translation + tf.rotation * spec.turret_pos;
        if self.kind == VehicleKind::Heli && self.weapons[self.active].kind == WeaponKind::Rockets {
            let dir = tf.rotation * Vec3::new(0.0, -0.04, -1.0).normalize();
            let side = if self.weapons[self.active].shots % 2 == 0 { 1.3 } else { -1.3 };
            return (tf.translation + tf.rotation * Vec3::new(side, 1.2, -1.4), dir);
        }
        let dir = tf.rotation
            * Quat::from_rotation_y(self.turret_yaw)
            * Quat::from_rotation_x(self.turret_pitch)
            * Vec3::NEG_Z;
        (pivot + dir * spec.barrel_len, dir)
    }

    pub fn center(&self, tf: &Transform) -> Vec3 {
        tf.translation + tf.rotation * Vec3::new(0.0, self.spec().half.y, 0.0)
    }
}

/// AI driver/gunner for a crewed vehicle.
#[derive(Component, Default)]
pub struct VehicleAi {
    pub route: Vec<Vec3>,
    pub idx: usize,
    pub looping: bool,
    pub dismount_at_end: bool,
    pub arrived: bool,
    pub target: Option<Entity>,
    pub target_pos: Vec3,
    pub scan: f32,
    pub stuck: f32,
    pub reverse: f32,
    pub aim_error: Vec3,
}

#[derive(Message, Clone, Copy)]
pub struct EnterVehicle {
    pub soldier: Entity,
    /// `None` picks the closest vehicle in reach.
    pub vehicle: Option<Entity>,
}

#[derive(Message, Clone, Copy)]
pub struct ExitVehicle {
    pub soldier: Entity,
}

#[derive(Resource)]
pub struct VehicleAssets {
    cube: Handle<Mesh>,
    cyl: Handle<Mesh>,
    sphere: Handle<Mesh>,
    paint: [[Handle<StandardMaterial>; 3]; 2],
    dark: Handle<StandardMaterial>,
    glass: Handle<StandardMaterial>,
    metal: Handle<StandardMaterial>,
    pub wreck: Handle<StandardMaterial>,
}

pub struct VehiclePlugin;

impl Plugin for VehiclePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<EnterVehicle>()
            .add_message::<ExitVehicle>()
            .add_systems(Startup, setup_vehicle_assets)
            .add_systems(
                Update,
                (vehicle_ai, handle_enter_exit)
                    .chain()
                    .in_set(Phase::Ai)
                    .run_if(is_authority),
            )
            .add_systems(
                Update,
                (ground_physics, heli_physics, run_over, sync_occupants)
                    .chain()
                    .in_set(Phase::Movement)
                    .run_if(is_authority),
            )
            .add_systems(
                Update,
                vehicle_weapons.in_set(Phase::Combat).run_if(is_authority),
            )
            .add_systems(
                Update,
                vehicle_killed
                    .after(crate::combat::apply_damage)
                    .in_set(Phase::Damage)
                    .run_if(is_authority),
            )
            .add_systems(
                Update,
                (animate_vehicles, wreck_visuals, vehicle_smoke, hide_occupants)
                    .in_set(Phase::Camera),
            );
    }
}

fn setup_vehicle_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut m = |c: Color, rough: f32, metallic: f32| {
        materials.add(StandardMaterial {
            base_color: c,
            perceptual_roughness: rough,
            metallic,
            ..default()
        })
    };
    let paint = [
        [
            m(Color::srgb(0.6, 0.54, 0.4), 0.7, 0.1),
            m(Color::srgb(0.55, 0.5, 0.38), 0.7, 0.1),
            m(Color::srgb(0.24, 0.27, 0.21), 0.6, 0.1),
        ],
        [
            m(Color::srgb(0.3, 0.35, 0.24), 0.7, 0.1),
            m(Color::srgb(0.27, 0.32, 0.22), 0.7, 0.1),
            m(Color::srgb(0.32, 0.36, 0.3), 0.6, 0.1),
        ],
    ];
    commands.insert_resource(VehicleAssets {
        cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        cyl: meshes.add(Cylinder::new(1.0, 1.0)),
        sphere: meshes.add(Sphere::new(1.0)),
        paint,
        dark: m(Color::srgb(0.08, 0.08, 0.08), 0.9, 0.0),
        glass: materials.add(StandardMaterial {
            base_color: Color::srgba(0.15, 0.2, 0.25, 0.6),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.05,
            metallic: 0.5,
            ..default()
        }),
        metal: m(Color::srgb(0.15, 0.15, 0.15), 0.4, 0.7),
        wreck: m(Color::srgb(0.07, 0.065, 0.06), 1.0, 0.0),
    });
}

struct ModelBuilder<'a, 'w, 's> {
    commands: &'a mut Commands<'w, 's>,
    assets: &'a VehicleAssets,
    parts: Vec<Entity>,
}

#[derive(Clone, Copy)]
enum M {
    Cube,
    Cyl,
    Sphere,
}

impl ModelBuilder<'_, '_, '_> {
    fn part(
        &mut self,
        mesh: M,
        mat: &Handle<StandardMaterial>,
        pos: Vec3,
        scale: Vec3,
        rot: Quat,
    ) -> Entity {
        let mesh = match mesh {
            M::Cube => self.assets.cube.clone(),
            M::Cyl => self.assets.cyl.clone(),
            M::Sphere => self.assets.sphere.clone(),
        };
        let e = self
            .commands
            .spawn((
                Mesh3d(mesh),
                MeshMaterial3d(mat.clone()),
                Transform {
                    translation: pos,
                    rotation: rot,
                    scale,
                },
            ))
            .id();
        self.parts.push(e);
        e
    }

    fn pivot(&mut self, pos: Vec3) -> Entity {
        self.commands
            .spawn((Transform::from_translation(pos), Visibility::default()))
            .id()
    }
}

/// Wheel/rotor cylinders lie along X by default after this rotation.
fn along_x() -> Quat {
    Quat::from_rotation_z(FRAC_PI_2)
}

fn along_z() -> Quat {
    Quat::from_rotation_x(FRAC_PI_2)
}

pub fn spawn_vehicle(
    commands: &mut Commands,
    assets: &VehicleAssets,
    kind: VehicleKind,
    paint: Faction,
    pos: Vec3,
    yaw: f32,
) -> Entity {
    let fi = if paint == Faction::Blufor { 0 } else { 1 };
    let body = assets.paint[fi][0].clone();
    let body2 = assets.paint[fi][1].clone();
    let heli_paint = assets.paint[fi][2].clone();
    let dark = assets.dark.clone();
    let glass = assets.glass.clone();
    let metal = assets.metal.clone();
    let mut b = ModelBuilder {
        commands,
        assets,
        parts: Vec::new(),
    };
    let i = Quat::IDENTITY;
    let mut children = Vec::new();
    let mut rotors = Vec::new();
    let turret;
    let gun;
    match kind {
        VehicleKind::Jeep => {
            children.push(b.part(M::Cube, &body, Vec3::new(0.0, 0.8, 0.0), Vec3::new(2.15, 0.55, 4.7), i));
            children.push(b.part(M::Cube, &body2, Vec3::new(0.0, 1.2, -1.55), Vec3::new(2.05, 0.4, 1.6), i));
            children.push(b.part(M::Cube, &body, Vec3::new(0.0, 1.45, 0.45), Vec3::new(2.15, 0.85, 2.6), i));
            children.push(b.part(M::Cube, &body2, Vec3::new(0.0, 1.95, 0.45), Vec3::new(2.1, 0.12, 2.4), i));
            children.push(b.part(
                M::Cube,
                &glass,
                Vec3::new(0.0, 1.62, -0.88),
                Vec3::new(1.9, 0.5, 0.05),
                Quat::from_rotation_x(-0.25),
            ));
            for (x, z) in [(-1.0, -1.5), (1.0, -1.5), (-1.0, 1.55), (1.0, 1.55)] {
                children.push(b.part(M::Cyl, &dark, Vec3::new(x, 0.46, z), Vec3::new(0.46, 0.36, 0.46), along_x()));
            }
            turret = b.pivot(JEEP.turret_pos);
            let shield = b.part(M::Cube, &body2, Vec3::new(0.0, 0.2, -0.35), Vec3::new(0.9, 0.55, 0.05), i);
            let ring = b.part(M::Cyl, &body2, Vec3::new(0.0, -0.12, 0.0), Vec3::new(0.55, 0.1, 0.55), i);
            gun = b.pivot(Vec3::new(0.0, 0.32, 0.0));
            let recv = b.part(M::Cube, &metal, Vec3::new(0.0, 0.0, 0.05), Vec3::new(0.16, 0.16, 0.55), i);
            let barrel = b.part(M::Cyl, &metal, Vec3::new(0.0, 0.0, -0.75), Vec3::new(0.035, 1.1, 0.035), along_z());
            b.commands.entity(gun).add_children(&[recv, barrel]);
            b.commands.entity(turret).add_children(&[shield, ring, gun]);
            children.push(turret);
        }
        VehicleKind::Ifv => {
            children.push(b.part(M::Cube, &body, Vec3::new(0.0, 1.05, 0.1), Vec3::new(3.2, 1.0, 6.3), i));
            children.push(b.part(
                M::Cube,
                &body2,
                Vec3::new(0.0, 1.3, -3.25),
                Vec3::new(3.2, 0.7, 1.3),
                Quat::from_rotation_x(-0.55),
            ));
            children.push(b.part(M::Cube, &body2, Vec3::new(0.0, 1.85, 0.4), Vec3::new(3.0, 0.65, 5.6), i));
            for x in [-1.6f32, 1.6] {
                children.push(b.part(M::Cube, &dark, Vec3::new(x, 0.55, 0.0), Vec3::new(0.62, 1.0, 6.8), i));
                for k in 0..6 {
                    let z = -2.6 + k as f32 * 1.04;
                    children.push(b.part(
                        M::Cyl,
                        &body2,
                        Vec3::new(x * 1.03, 0.42, z),
                        Vec3::new(0.38, 0.2, 0.38),
                        along_x(),
                    ));
                }
            }
            turret = b.pivot(IFV.turret_pos);
            let tbody = b.part(M::Cube, &body, Vec3::new(0.0, 0.35, 0.2), Vec3::new(2.0, 0.7, 2.6), i);
            let hatch = b.part(M::Cyl, &body2, Vec3::new(0.45, 0.75, 0.6), Vec3::new(0.4, 0.12, 0.4), i);
            let sight = b.part(M::Cube, &metal, Vec3::new(-0.7, 0.85, -0.3), Vec3::new(0.35, 0.3, 0.45), i);
            gun = b.pivot(Vec3::new(0.0, 0.35, -1.1));
            let mantlet = b.part(M::Cube, &body2, Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.7, 0.45, 0.4), i);
            let barrel = b.part(M::Cyl, &metal, Vec3::new(0.0, 0.0, -1.5), Vec3::new(0.07, 2.6, 0.07), along_z());
            let coax = b.part(M::Cyl, &metal, Vec3::new(0.3, 0.05, -0.5), Vec3::new(0.03, 0.6, 0.03), along_z());
            b.commands.entity(gun).add_children(&[mantlet, barrel, coax]);
            b.commands.entity(turret).add_children(&[tbody, hatch, sight, gun]);
            children.push(turret);
        }
        VehicleKind::Heli => {
            let p = &heli_paint;
            children.push(b.part(M::Cube, p, Vec3::new(0.0, 1.6, -0.1), Vec3::new(1.9, 1.7, 4.4), i));
            children.push(b.part(M::Sphere, p, Vec3::new(0.0, 1.35, -2.3), Vec3::new(0.95, 0.8, 1.2), i));
            children.push(b.part(M::Sphere, &glass, Vec3::new(0.0, 1.85, -2.1), Vec3::new(0.88, 0.7, 0.95), i));
            children.push(b.part(M::Cube, p, Vec3::new(0.0, 2.05, 4.4), Vec3::new(0.45, 0.5, 5.0), i));
            children.push(b.part(M::Cube, p, Vec3::new(0.0, 2.65, 6.75), Vec3::new(0.12, 1.4, 0.9), i));
            children.push(b.part(M::Cube, p, Vec3::new(0.0, 2.05, 6.0), Vec3::new(2.0, 0.08, 0.5), i));
            children.push(b.part(M::Cube, p, Vec3::new(0.0, 2.75, 0.3), Vec3::new(1.2, 0.6, 2.2), i));
            for x in [-1.0f32, 1.0] {
                children.push(b.part(M::Cube, &dark, Vec3::new(x, 0.06, -0.2), Vec3::new(0.12, 0.12, 4.2), i));
                for z in [-1.2f32, 1.0] {
                    children.push(b.part(M::Cube, &dark, Vec3::new(x * 0.9, 0.38, z), Vec3::new(0.08, 0.65, 0.08), i));
                }
                children.push(b.part(M::Cube, p, Vec3::new(x * 1.05, 1.3, -0.4), Vec3::new(0.9, 0.08, 0.6), i));
                children.push(b.part(M::Cyl, &dark, Vec3::new(x * 1.35, 1.15, -0.4), Vec3::new(0.2, 1.4, 0.2), along_z()));
            }
            let main_rotor = b.pivot(Vec3::new(0.0, 3.2, 0.1));
            let hub = b.part(M::Cyl, &dark, Vec3::ZERO, Vec3::new(0.25, 0.3, 0.25), i);
            let blade1 = b.part(M::Cube, &dark, Vec3::Y * 0.1, Vec3::new(0.35, 0.04, 14.0), i);
            let blade2 = b.part(M::Cube, &dark, Vec3::Y * 0.1, Vec3::new(14.0, 0.04, 0.35), i);
            b.commands.entity(main_rotor).add_children(&[hub, blade1, blade2]);
            let tail_rotor = b.pivot(Vec3::new(0.25, 2.75, 6.9));
            let t1 = b.part(M::Cube, &dark, Vec3::ZERO, Vec3::new(0.04, 2.2, 0.2), i);
            let t2 = b.part(M::Cube, &dark, Vec3::ZERO, Vec3::new(0.04, 0.2, 2.2), i);
            b.commands.entity(tail_rotor).add_children(&[t1, t2]);
            children.push(main_rotor);
            children.push(tail_rotor);
            rotors.push((main_rotor, Vec3::Y));
            rotors.push((tail_rotor, Vec3::X));
            turret = b.pivot(HELI.turret_pos);
            let ball = b.part(M::Sphere, &dark, Vec3::ZERO, Vec3::splat(0.25), i);
            gun = b.pivot(Vec3::ZERO);
            let barrel = b.part(M::Cyl, &metal, Vec3::new(0.0, 0.0, -0.45), Vec3::new(0.06, 0.8, 0.06), along_z());
            b.commands.entity(gun).add_child(barrel);
            b.commands.entity(turret).add_children(&[ball, gun]);
            children.push(turret);
        }
    }
    let parts = std::mem::take(&mut b.parts);
    let spec = match kind {
        VehicleKind::Jeep => &JEEP,
        VehicleKind::Ifv => &IFV,
        VehicleKind::Heli => &HELI,
    };
    let root = commands
        .spawn((
            Vehicle {
                kind,
                paint,
                driver: None,
                passengers: Vec::new(),
                crew_faction: None,
                speed: 0.0,
                velocity: Vec3::ZERO,
                yaw,
                pitch: 0.0,
                roll: 0.0,
                input: VehicleInput::default(),
                weapons: spec
                    .weapons
                    .iter()
                    .map(|&(k, mags)| Weapon::new(k, mags))
                    .collect(),
                active: 0,
                turret_yaw: 0.0,
                turret_pitch: 0.0,
                destroyed: false,
                wrecked_visual: false,
                rotor: 0.0,
                airborne: false,
                smoke_timer: 0.0,
                turret,
                gun,
                rotors,
                parts,
            },
            Health::new(spec.hp, spec.armor),
            Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
            Visibility::default(),
            MissionEntity,
        ))
        .id();
    commands.entity(root).add_children(&children);
    root
}

fn handle_enter_exit(
    mut enters: MessageReader<EnterVehicle>,
    mut exits: MessageReader<ExitVehicle>,
    mut soldiers: Query<(&mut Soldier, &mut Transform, &Health), Without<Vehicle>>,
    mut vehicles: Query<(Entity, &mut Vehicle, &Transform, &Health)>,
    terrain: Res<Terrain>,
    obstacles: Res<Obstacles>,
    mut sounds: MessageWriter<PlaySound>,
) {
    for ev in exits.read() {
        let Ok((mut s, mut stf, _)) = soldiers.get_mut(ev.soldier) else {
            continue;
        };
        let Some(ve) = s.vehicle else { continue };
        let Ok((_, mut v, vtf, _)) = vehicles.get_mut(ve) else {
            s.vehicle = None;
            continue;
        };
        let ground = terrain.height(vtf.translation.x, vtf.translation.z);
        if v.kind == VehicleKind::Heli && vtf.translation.y - ground > 4.0 && !v.destroyed {
            continue;
        }
        v.remove_occupant(ev.soldier);
        s.vehicle = None;
        let half = v.spec().half;
        let (yaw, _, _) = vtf.rotation.to_euler(EulerRot::YXZ);
        let right = right_of_yaw(yaw);
        let fwd = forward_of_yaw(yaw);
        // Try both sides, then front and back, for a free spot.
        let mut spot = vtf.translation + right * (half.x + 1.0);
        for cand in [
            right * (half.x + 1.0),
            -right * (half.x + 1.0),
            -fwd * (half.z + 1.2),
            fwd * (half.z + 1.2),
        ] {
            let p = vtf.translation + cand;
            if obstacles.overlap(p, 0.35, 1.8).is_none() {
                spot = p;
                break;
            }
        }
        spot.y = terrain.height(spot.x, spot.z);
        stf.translation = spot;
        s.yaw = yaw;
        s.stance = crate::soldier::Stance::Stand;
        sounds.write(PlaySound::new(SoundKind::Door, spot));
    }
    for ev in enters.read() {
        let Ok((mut s, stf, health)) = soldiers.get_mut(ev.soldier) else {
            continue;
        };
        if health.dead || s.vehicle.is_some() {
            continue;
        }
        let pos = stf.translation;
        let mut best: Option<(f32, Entity)> = None;
        for (e, v, vtf, vh) in &vehicles {
            if vh.dead || v.destroyed || !v.has_seat() {
                continue;
            }
            if v.crew_faction.is_some_and(|f| f != s.faction) {
                continue;
            }
            if let Some(want) = ev.vehicle
                && want != e
            {
                continue;
            }
            let half = v.spec().half;
            let local = vtf.rotation.inverse() * (pos - v.center(vtf));
            let outside = (local.abs() - half).max(Vec3::ZERO).length();
            let reach = if ev.vehicle.is_some() { 6.0 } else { 2.5 };
            if outside < reach && best.is_none_or(|(d, _)| outside < d) {
                best = Some((outside, e));
            }
        }
        let Some((_, ve)) = best else { continue };
        let Ok((_, mut v, vtf, _)) = vehicles.get_mut(ve) else {
            continue;
        };
        // AI boarding a squad vehicle prefers the back seats.
        if v.driver.is_none() && (ev.vehicle.is_none() || v.passengers.len() + 1 >= v.spec().seats) {
            v.driver = Some(ev.soldier);
        } else if v.passengers.len() + 1 < v.spec().seats {
            v.passengers.push(ev.soldier);
        } else if v.driver.is_none() {
            v.driver = Some(ev.soldier);
        } else {
            continue;
        }
        v.crew_faction = Some(s.faction);
        s.vehicle = Some(ve);
        s.velocity = Vec3::ZERO;
        sounds.write(PlaySound::new(SoundKind::Door, vtf.translation));
    }
}

fn ground_physics(
    time: Res<Time>,
    terrain: Res<Terrain>,
    obstacles: Res<Obstacles>,
    mut vehicles: Query<(Entity, &mut Vehicle, &mut Transform, &mut Health)>,
    mut falls: MessageWriter<TreeFell>,
) {
    let dt = time.delta_secs().min(0.05);
    let mut circles: Vec<(Entity, Vec3, f32)> = vehicles
        .iter()
        .map(|(e, v, tf, _)| (e, tf.translation, v.spec().half.x.max(1.0)))
        .collect();
    for (e, mut v, mut tf, mut health) in &mut vehicles {
        if v.kind == VehicleKind::Heli {
            continue;
        }
        let spec = v.spec();
        let inp = if v.driver.is_some() && !v.destroyed {
            v.input.clone()
        } else {
            VehicleInput {
                brake: true,
                ..default()
            }
        };
        let max_rev = spec.max_speed * 0.35;
        if inp.throttle.abs() > 0.05 {
            let target = if inp.throttle > 0.0 {
                inp.throttle * spec.max_speed
            } else {
                inp.throttle * max_rev
            };
            let braking = v.speed.signum() != target.signum() && v.speed.abs() > 0.5;
            let a = if braking { spec.accel * 2.5 } else { spec.accel };
            v.speed += (target - v.speed).clamp(-a * dt, a * dt);
        } else {
            let decel = if inp.brake { 10.0 } else { 2.5 };
            v.speed -= v.speed.signum() * (decel * dt).min(v.speed.abs());
        }
        // Gravity along the slope.
        v.speed -= GRAVITY * v.pitch.sin() * dt * 0.6;

        let steer_k = if spec.tracked {
            if v.speed.abs() < 1.0 { 1.0 } else { v.speed.signum() }
        } else {
            (v.speed / 4.0).clamp(-1.0, 1.0) * (1.0 - 0.5 * v.speed.abs() / spec.max_speed)
        };
        let yaw_rate = -inp.steer * spec.turn_rate * steer_k;
        let new_yaw = v.yaw + yaw_rate * dt;
        let fwd = forward_of_yaw(new_yaw);
        let pos = tf.translation;
        let mut new = pos + fwd * v.speed * dt;
        new.x = new.x.clamp(-MAP_HALF + 10.0, MAP_HALF - 10.0);
        new.z = new.z.clamp(-MAP_HALF + 10.0, MAP_HALF - 10.0);

        // Collision probes along the hull outline.
        let right = right_of_yaw(new_yaw);
        let half = spec.half;
        let mut blocked = false;
        let probes = [
            fwd * half.z + right * half.x * 0.8,
            fwd * half.z - right * half.x * 0.8,
            fwd * half.z,
            -fwd * half.z + right * half.x * 0.8,
            -fwd * half.z - right * half.x * 0.8,
            right * half.x,
            -right * half.x,
        ];
        let ground_here = terrain.height(new.x, new.z);
        for off in probes {
            let p = Vec3::new(new.x + off.x, ground_here + 0.4, new.z + off.z);
            if let Some(i) = obstacles.overlap(p, 0.35, 1.2) {
                let o = &obstacles.list[i];
                let heavy = spec.tracked || v.speed.abs() > 9.0;
                if o.tree && matches!(o.shape, Shape::Cyl { .. }) && heavy {
                    falls.write(TreeFell {
                        obstacle: i as u32,
                        dir: Vec2::new(fwd.x, fwd.z) * v.speed.signum(),
                    });
                    v.speed *= if spec.tracked { 0.85 } else { 0.5 };
                } else {
                    blocked = true;
                }
            }
        }
        for &(oe, op, or) in &circles {
            if oe == e {
                continue;
            }
            let d = Vec2::new(new.x - op.x, new.z - op.z);
            if d.length() < or + half.x + 1.0 && d.dot(Vec2::new(fwd.x, fwd.z) * v.speed) < 0.0 {
                blocked = true;
            }
        }
        if terrain.height(new.x, new.z) < WATER_LEVEL - 1.2 {
            blocked = true;
        }
        if blocked {
            if v.speed.abs() > 8.0 {
                health.hp -= (v.speed.abs() - 8.0) * 12.0;
            }
            v.speed *= -0.2;
            new = pos;
        } else {
            v.yaw = new_yaw;
        }

        // Settle on the terrain.
        let fwd = forward_of_yaw(v.yaw);
        let right = right_of_yaw(v.yaw);
        let lz = half.z * 0.8;
        let lx = half.x * 0.9;
        let h = |p: Vec3| terrain.height(p.x, p.z);
        let hf = h(new + fwd * lz);
        let hb = h(new - fwd * lz);
        let hr = h(new + right * lx);
        let hl = h(new - right * lx);
        let target_pitch = f32::atan2(hf - hb, 2.0 * lz);
        let target_roll = f32::atan2(hr - hl, 2.0 * lx);
        let k = (dt * 8.0).min(1.0);
        v.pitch += (target_pitch - v.pitch) * k;
        v.roll += (target_roll - v.roll) * k;
        let ground = ((hf + hb + hr + hl) * 0.25).max(h(new));
        new.y += (ground - new.y) * (dt * 12.0).min(1.0);
        if (new.y - ground).abs() > 2.0 {
            new.y = ground;
        }
        if v.pitch.abs() > 0.6 && v.speed * v.pitch.signum() > 0.0 {
            v.speed *= 1.0 - dt * 2.0;
        }
        v.velocity = (new - pos) / dt.max(1e-4);
        tf.translation = new;
        tf.rotation = Quat::from_euler(EulerRot::YXZ, v.yaw, v.pitch, v.roll);
        if let Some(c) = circles.iter_mut().find(|c| c.0 == e) {
            c.1 = new;
        }
    }
}

fn heli_physics(
    time: Res<Time>,
    terrain: Res<Terrain>,
    obstacles: Res<Obstacles>,
    mut vehicles: Query<(Entity, &mut Vehicle, &mut Transform, &mut Health)>,
    mut damage: MessageWriter<Damage>,
) {
    let dt = time.delta_secs().min(0.05);
    for (e, mut v, mut tf, mut health) in &mut vehicles {
        if v.kind != VehicleKind::Heli {
            continue;
        }
        let crewed = v.driver.is_some() && !v.destroyed;
        let inp = if crewed {
            v.input.clone()
        } else {
            VehicleInput::default()
        };
        v.rotor = if crewed {
            (v.rotor + dt * 0.2).min(1.0)
        } else {
            (v.rotor - dt * 0.08).max(0.0)
        };
        let k = (dt * 2.2).min(1.0);
        let target_pitch = -inp.cyclic_pitch * 0.42;
        let target_roll = -inp.cyclic_roll * 0.45;
        v.pitch += (target_pitch - v.pitch) * k;
        v.roll += (target_roll - v.roll) * k;
        // Banking turns the aircraft (coordinated turn), pedals add yaw.
        let yaw_rate = inp.pedal * -1.2 + v.roll * 1.4 * (v.velocity.length() / 30.0).min(1.0);
        v.yaw += yaw_rate * dt;
        let rot = Quat::from_euler(EulerRot::YXZ, v.yaw, v.pitch, v.roll);
        let up = rot * Vec3::Y;
        let thrust = if v.destroyed {
            0.0
        } else {
            GRAVITY * (1.0 + inp.collective * 0.7) / up.y.max(0.6) * v.rotor
        };
        let mut accel = up * thrust - Vec3::Y * GRAVITY;
        let vel = v.velocity;
        accel -= Vec3::new(vel.x * 0.12, vel.y * 0.6, vel.z * 0.12);
        v.velocity += accel * dt;
        let pos = tf.translation;
        let mut new = pos + v.velocity * dt;
        new.x = new.x.clamp(-MAP_HALF + 10.0, MAP_HALF - 10.0);
        new.z = new.z.clamp(-MAP_HALF + 10.0, MAP_HALF - 10.0);
        new.y = new.y.min(1500.0);
        let ground = terrain.height(new.x, new.z).max(WATER_LEVEL - 2.0);
        let speed = v.velocity.length();
        let mut crash = false;
        if new.y <= ground {
            let impact = -v.velocity.y;
            let tilt = up.angle_between(Vec3::Y);
            if !v.destroyed && (impact > 7.0 || tilt > 0.55 || Vec2::new(v.velocity.x, v.velocity.z).length() > 14.0) {
                crash = true;
            }
            if ground <= WATER_LEVEL - 1.0 && !v.destroyed {
                crash = true;
            }
            new.y = ground;
            v.velocity.y = v.velocity.y.max(0.0);
            v.velocity.x *= 1.0 - (dt * 4.0).min(1.0);
            v.velocity.z *= 1.0 - (dt * 4.0).min(1.0);
            v.airborne = false;
            if inp.collective <= 0.0 {
                v.pitch *= 1.0 - (dt * 3.0).min(1.0);
                v.roll *= 1.0 - (dt * 3.0).min(1.0);
            }
        } else {
            v.airborne = new.y - ground > 1.0;
        }
        let hull = new + rot * Vec3::Y * 1.4;
        if v.airborne
            && let Some((_, i)) = obstacles.segment_hit(pos + Vec3::Y * 1.4, hull, MOVE)
        {
            let _ = i;
            if speed > 5.0 && !v.destroyed {
                crash = true;
            }
            new = pos;
            v.velocity *= -0.2;
        }
        if crash {
            health.hp = 0.0;
            damage.write(Damage {
                target: e,
                amount: 10_000.0,
                ap: 3,
                attacker: v.driver,
                headshot: false,
            });
        }
        v.speed = speed;
        tf.translation = new;
        tf.rotation = rot;
    }
}

fn run_over(
    vehicles: Query<(Entity, &Vehicle, &Transform)>,
    soldiers: Query<(Entity, &Soldier, &Transform, &Health), Without<Vehicle>>,
    mut damage: MessageWriter<Damage>,
) {
    for (ve, v, vtf) in &vehicles {
        if v.kind == VehicleKind::Heli || v.speed.abs() < 3.5 {
            continue;
        }
        let half = v.spec().half + Vec3::new(0.3, 0.0, 0.3);
        let center = v.center(vtf);
        for (se, s, stf, h) in &soldiers {
            if h.dead || s.vehicle.is_some() {
                continue;
            }
            let p = stf.translation + Vec3::Y * 0.5;
            if p.distance_squared(center) > half.length_squared() {
                continue;
            }
            let local = vtf.rotation.inverse() * (p - center);
            if local.x.abs() < half.x && local.z.abs() < half.z && local.y.abs() < half.y + 0.5 {
                let _ = ve;
                damage.write(Damage {
                    target: se,
                    amount: 60.0 * v.speed.abs(),
                    ap: 1,
                    attacker: v.driver,
                    headshot: false,
                });
            }
        }
    }
}

fn sync_occupants(
    vehicles: Query<(&Vehicle, &Transform)>,
    mut soldiers: Query<(&Soldier, &mut Transform), Without<Vehicle>>,
) {
    for (s, mut tf) in &mut soldiers {
        if let Some(ve) = s.vehicle
            && let Ok((v, vtf)) = vehicles.get(ve)
        {
            tf.translation = vtf.translation + Vec3::Y * v.spec().half.y;
        }
    }
}

/// Elevation needed to hit a target at `dist` (flat-fire approximation with drag).
pub fn drop_compensation(kind: WeaponKind, dist: f32) -> f32 {
    let st = kind.stats();
    let v = st.muzzle_velocity;
    let mut t = dist / v;
    for _ in 0..2 {
        t = dist / (v * (-st.drag * t * 0.5).exp());
    }
    let drop = 0.5 * GRAVITY * st.gravity * t * t;
    (drop / dist.max(1.0)).atan()
}

fn vehicle_weapons(
    time: Res<Time>,
    mut vehicles: Query<(Entity, &mut Vehicle, &Transform)>,
    soldiers: Query<&Soldier>,
    mut shooter: Shooter,
) {
    let dt = time.delta_secs();
    for (e, mut v, tf) in &mut vehicles {
        for w in v.weapons.iter_mut() {
            w.tick(dt);
        }
        if v.destroyed || v.driver.is_none() {
            continue;
        }
        let inp = std::mem::take(&mut v.input.switch_weapon);
        if inp {
            v.active = (v.active + 1) % v.weapons.len();
        }
        let spec = v.spec();
        if let Some(target) = v.input.aim_point {
            let pivot = tf.translation + tf.rotation * spec.turret_pos;
            let dist = pivot.distance(target);
            let kind = v.weapons[v.active].kind;
            let mut dir = (target - pivot).normalize_or(Vec3::NEG_Z);
            let (yw, pw) = angles_of(dir);
            dir = dir_from_angles(yw, pw + drop_compensation(kind, dist));
            let local = tf.rotation.inverse() * dir;
            let (ty, tp) = angles_of(local);
            v.turret_yaw = turn_toward(v.turret_yaw, ty, spec.turret_speed * dt);
            let (lo, hi) = spec.pitch_limits;
            let tp = tp.clamp(lo, hi);
            v.turret_pitch += (tp - v.turret_pitch).clamp(-spec.turret_speed * dt, spec.turret_speed * dt);
        }
        if !v.input.fire {
            continue;
        }
        let faction = v
            .driver
            .and_then(|d| soldiers.get(d).ok())
            .map(|s| s.faction)
            .unwrap_or(v.paint);
        let active = v.active;
        let w = &mut v.weapons[active];
        if w.ammo == 0 {
            if w.start_reload() {
                shooter.sounds.write(PlaySound::new(SoundKind::Reload, tf.translation));
            }
            continue;
        }
        if !w.try_fire() {
            continue;
        }
        let tracer = w.is_tracer();
        let kind = w.kind;
        let spread = kind.stats().spread * kind.stats().ads_factor;
        let (muzzle, dir) = v.barrel(tf);
        let dir = apply_spread(dir, spread);
        let shooter_entity = v.driver.unwrap_or(e);
        let vel = v.velocity;
        shooter.fire(kind, muzzle, dir, vel, shooter_entity, Some(e), faction, tracer);
    }
}

fn vehicle_killed(
    mut events: MessageReader<Killed>,
    mut vehicles: Query<(&mut Vehicle, &Transform)>,
    mut damage: MessageWriter<Damage>,
    mut fx: MessageWriter<ExplosionFx>,
    mut sounds: MessageWriter<PlaySound>,
    mut radio: MessageWriter<Radio>,
) {
    for ev in events.read() {
        let Ok((mut v, tf)) = vehicles.get_mut(ev.victim) else {
            continue;
        };
        if v.destroyed {
            continue;
        }
        v.destroyed = true;
        v.speed = 0.0;
        let center = v.center(tf);
        fx.write(ExplosionFx { pos: center, size: 9.0 });
        sounds.write(PlaySound::new(SoundKind::Explosion, center).volume(2.0));
        for occ in v.occupants().collect::<Vec<_>>() {
            damage.write(Damage {
                target: occ,
                amount: 1000.0,
                ap: 3,
                attacker: ev.attacker,
                headshot: false,
            });
        }
        radio.write(Radio::new(format!("{} destroyed", v.name()), RADIO_SYSTEM));
    }
}

fn vehicle_ai(
    time: Res<Time>,
    targets: Res<Targets>,
    terrain: Res<Terrain>,
    obstacles: Res<Obstacles>,
    mut vehicles: Query<(Entity, &mut Vehicle, &mut VehicleAi, &Transform)>,
    soldiers: Query<&Soldier>,
    humans: Query<(), With<crate::player::Human>>,
    mut exits: MessageWriter<ExitVehicle>,
) {
    let dt = time.delta_secs();
    for (e, mut v, mut ai, tf) in &mut vehicles {
        let Some(driver) = v.driver else { continue };
        if v.destroyed || humans.contains(driver) {
            continue;
        }
        let Ok(ds) = soldiers.get(driver) else { continue };
        let faction = ds.faction;
        let spec = v.spec();
        let pos = tf.translation;

        // Targeting.
        ai.scan -= dt;
        if ai.scan <= 0.0 {
            ai.scan = 0.6 + fastrand::f32() * 0.4;
            let eye = tf.translation + tf.rotation * spec.turret_pos + Vec3::Y * 0.4;
            let mut best: Option<(f32, Entity, Vec3)> = None;
            for t in &targets.list {
                if t.faction != faction.enemy() || t.dead {
                    continue;
                }
                let d = t.pos.distance(pos);
                let range = if t.is_vehicle { 900.0 } else { 550.0 } * t.visibility;
                if d > range {
                    continue;
                }
                let score = d * if t.is_vehicle { 0.6 } else { 1.0 };
                if best.is_some_and(|(s, _, _)| score >= s) {
                    continue;
                }
                if line_of_sight(&terrain, &obstacles, eye, t.aim) {
                    best = Some((score, t.entity, t.aim));
                }
            }
            let new_target = best.map(|b| b.1);
            if new_target != ai.target {
                ai.aim_error = Vec3::new(rand_range(-1.0, 1.0), rand_range(-0.5, 1.0), rand_range(-1.0, 1.0)) * 4.0;
            }
            ai.target = new_target;
        }
        let target = ai
            .target
            .and_then(|t| targets.list.iter().find(|x| x.entity == t));
        if let Some(t) = target {
            let wants_cannon = t.is_vehicle || t.pos.distance(pos) > 300.0;
            let preferred = v
                .weapons
                .iter()
                .position(|w| {
                    let explosive = w.kind.is_explosive();
                    explosive == wants_cannon && (w.ammo > 0 || w.mags > 0)
                })
                .unwrap_or(0);
            v.active = preferred;
            let kind = v.weapons[preferred].kind;
            let muzzle = tf.translation + tf.rotation * spec.turret_pos;
            let aim = ballistic_aim(muzzle, t.aim, t.vel, kind) + ai.aim_error;
            // The vehicle's own turret logic adds drop compensation again; remove it here.
            let dist = muzzle.distance(aim);
            let dir = (aim - muzzle).normalize_or(Vec3::NEG_Z);
            let (yw, pw) = angles_of(dir);
            let flat = dir_from_angles(yw, pw - drop_compensation(kind, dist));
            v.input.aim_point = Some(muzzle + flat * dist);
            ai.aim_error *= 1.0 - (dt * 0.6).min(1.0);
            let (_, bdir) = v.barrel(tf);
            v.input.fire = bdir.angle_between(dir) < 0.03;
        } else {
            v.input.fire = false;
            v.input.aim_point = Some(tf.translation + tf.rotation * (spec.turret_pos + Vec3::new(0.0, 0.0, -50.0)));
        }

        // Driving.
        if ai.arrived || ai.route.is_empty() {
            v.input.throttle = 0.0;
            v.input.brake = true;
            if ai.arrived && ai.dismount_at_end && !v.passengers.is_empty() {
                for p in v.passengers.clone() {
                    exits.write(ExitVehicle { soldier: p });
                }
            }
            continue;
        }
        let wp = ai.route[ai.idx];
        let to = Vec2::new(wp.x - pos.x, wp.z - pos.z);
        if to.length() < 12.0 {
            ai.idx += 1;
            if ai.idx >= ai.route.len() {
                if ai.looping {
                    ai.idx = 0;
                } else {
                    ai.arrived = true;
                    ai.idx = ai.route.len() - 1;
                }
            }
            continue;
        }
        let desired_yaw = f32::atan2(-to.x, -to.y);
        let err = angle_diff(v.yaw, desired_yaw);
        if ai.reverse > 0.0 {
            ai.reverse -= dt;
            v.input.throttle = -0.6;
            v.input.steer = err.signum();
            v.input.brake = false;
            continue;
        }
        // Halt to fight when the target is close; tanks stop to shoot.
        let engaging = target.is_some_and(|t| t.pos.distance(pos) < 250.0) && v.kind == VehicleKind::Ifv;
        v.input.steer = (-err * 1.5).clamp(-1.0, 1.0);
        v.input.throttle = if engaging {
            0.0
        } else if err.abs() > 1.0 {
            0.3
        } else {
            0.75
        };
        v.input.brake = engaging;
        if v.speed.abs() < 0.5 && v.input.throttle > 0.0 {
            ai.stuck += dt;
            if ai.stuck > 2.5 {
                ai.stuck = 0.0;
                ai.reverse = 2.0;
            }
        } else {
            ai.stuck = 0.0;
        }
        let _ = e;
    }
}

fn animate_vehicles(
    time: Res<Time>,
    vehicles: Query<&Vehicle>,
    mut parts: Query<&mut Transform, Without<Vehicle>>,
) {
    let dt = time.delta_secs();
    for v in &vehicles {
        if let Ok(mut t) = parts.get_mut(v.turret) {
            t.rotation = Quat::from_rotation_y(v.turret_yaw);
        }
        if let Ok(mut t) = parts.get_mut(v.gun) {
            t.rotation = Quat::from_rotation_x(v.turret_pitch);
        }
        for &(r, axis) in &v.rotors {
            if let Ok(mut t) = parts.get_mut(r) {
                let speed = if axis == Vec3::Y { 28.0 } else { 45.0 };
                t.rotation = Quat::from_axis_angle(axis, v.rotor * speed * dt) * t.rotation;
            }
        }
    }
}

fn wreck_visuals(
    assets: Res<VehicleAssets>,
    mut vehicles: Query<&mut Vehicle>,
    mut mats: Query<&mut MeshMaterial3d<StandardMaterial>>,
) {
    for mut v in &mut vehicles {
        if v.destroyed && !v.wrecked_visual {
            v.wrecked_visual = true;
            for &p in &v.parts {
                if let Ok(mut m) = mats.get_mut(p) {
                    m.0 = assets.wreck.clone();
                }
            }
        }
    }
}

fn vehicle_smoke(
    mut commands: Commands,
    time: Res<Time>,
    fx: Res<FxAssets>,
    mut vehicles: Query<(&mut Vehicle, &Transform, &Health)>,
) {
    let dt = time.delta_secs();
    for (mut v, tf, h) in &mut vehicles {
        let ratio = h.hp / h.max;
        if !v.destroyed && ratio > 0.4 {
            continue;
        }
        v.smoke_timer -= dt;
        if v.smoke_timer > 0.0 {
            continue;
        }
        v.smoke_timer = if v.destroyed { 0.12 } else { 0.35 };
        let top = v.center(tf) + Vec3::Y * v.spec().half.y;
        let jitter = Vec3::new(rand_range(-0.5, 0.5), 0.0, rand_range(-0.5, 0.5));
        let mat = if v.destroyed { &fx.smoke_dark } else { &fx.smoke_light };
        spawn_puff(
            &mut commands,
            &fx,
            mat,
            top + jitter,
            rand_range(0.5, 0.9),
            rand_range(3.0, 5.0),
            Vec3::new(1.0, 3.5, 0.4) + jitter,
            2.5,
        );
        if v.destroyed && fastrand::f32() < 0.5 {
            spawn_puff(&mut commands, &fx, &fx.fire, top + jitter - Vec3::Y * 0.6, 0.6, 0.6, Vec3::Y * 2.0, 0.5);
        }
    }
}

fn hide_occupants(
    mut soldiers: Query<(&Soldier, &mut Visibility, Has<crate::common::Player>)>,
    view: Res<crate::player::LocalView>,
) {
    for (s, mut vis, is_local) in &mut soldiers {
        let hidden = s.vehicle.is_some() || (is_local && !view.third_person && !view.dead);
        let want = if hidden {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        if *vis != want {
            *vis = want;
        }
    }
}

#[allow(dead_code)]
fn no_shadows(commands: &mut Commands, e: Entity) {
    commands.entity(e).insert(NotShadowCaster);
}
