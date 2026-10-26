//! Infantry: components, models, movement, weapon handling and death.

use std::f32::consts::FRAC_PI_2;

use bevy::{light::NotShadowCaster, prelude::*};

use crate::audio::{PlaySound, SoundKind};
use crate::combat::{Shooter, Weapon, WeaponKind, apply_spread, spawn_projectile};
use crate::common::*;
use crate::net::is_authority;
use crate::terrain::{MAP_HALF, Terrain};
use crate::vehicle::Vehicle;
use crate::world::{Obstacle, Obstacles, SOLID, Shape};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Stance {
    Stand,
    Crouch,
    Prone,
}

impl Stance {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Stance::Crouch,
            2 => Stance::Prone,
            _ => Stance::Stand,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Stance::Stand => "STANDING",
            Stance::Crouch => "CROUCHED",
            Stance::Prone => "PRONE",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Role {
    Leader,
    Rifleman,
    Autorifleman,
    AtSoldier,
    Crew,
}

impl Role {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Role::Leader,
            2 => Role::Autorifleman,
            3 => Role::AtSoldier,
            4 => Role::Crew,
            _ => Role::Rifleman,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Role::Leader => "Team Leader",
            Role::Rifleman => "Rifleman",
            Role::Autorifleman => "Autorifleman",
            Role::AtSoldier => "AT Specialist",
            Role::Crew => "Crewman",
        }
    }
}

/// What a controller (human or AI) wants the soldier to do this frame.
#[derive(Clone, Default, Debug)]
pub struct Intent {
    /// World-space horizontal direction, length 0..=1.
    pub move_dir: Vec3,
    pub sprint: bool,
    pub fire: bool,
    pub ads: bool,
    pub reload: bool,
    pub throw: bool,
    pub heal: bool,
    pub toggle_mode: bool,
    pub switch_to: Option<usize>,
    pub stance: Option<Stance>,
}

#[derive(Component)]
pub struct Soldier {
    pub name: String,
    pub faction: Faction,
    pub role: Role,
    pub stance: Stance,
    pub yaw: f32,
    pub pitch: f32,
    pub velocity: Vec3,
    pub vehicle: Option<Entity>,
    pub squad: Option<Entity>,
    pub suppression: f32,
    pub threat_from: Option<Vec3>,
    pub stamina: f32,
    pub since_shot: f32,
    pub walk_phase: f32,
    pub intent: Intent,
    pub fak: u32,
    pub healing: f32,
    pub sprinting: bool,
    /// Weapon in hands (replicated for the model).
    pub held: WeaponKind,
    pub dry_clicked: bool,
}

impl Soldier {
    pub fn new(name: String, faction: Faction, role: Role, yaw: f32) -> Self {
        Self {
            name,
            faction,
            role,
            stance: Stance::Stand,
            yaw,
            pitch: 0.0,
            velocity: Vec3::ZERO,
            vehicle: None,
            squad: None,
            suppression: 0.0,
            threat_from: None,
            stamina: 1.0,
            since_shot: 99.0,
            walk_phase: 0.0,
            intent: Intent::default(),
            fak: if role == Role::Leader { 2 } else { 1 },
            healing: 0.0,
            sprinting: false,
            held: WeaponKind::Rifle,
            dry_clicked: false,
        }
    }

    pub fn eye(&self, pos: Vec3) -> Vec3 {
        match self.stance {
            Stance::Stand => pos + Vec3::Y * 1.62,
            Stance::Crouch => pos + Vec3::Y * 1.08,
            Stance::Prone => pos + Vec3::Y * 0.42 + forward_of_yaw(self.yaw) * 0.75,
        }
    }

    pub fn chest(&self, pos: Vec3) -> Vec3 {
        match self.stance {
            Stance::Stand => pos + Vec3::Y * 1.3,
            Stance::Crouch => pos + Vec3::Y * 0.85,
            Stance::Prone => pos + Vec3::Y * 0.25 + forward_of_yaw(self.yaw) * 0.3,
        }
    }

    pub fn aim_dir(&self) -> Vec3 {
        dir_from_angles(self.yaw, self.pitch)
    }

    /// Ray parameter and whether it was a headshot.
    pub fn hit_test(&self, pos: Vec3, a: Vec3, b: Vec3) -> Option<(f32, bool)> {
        let head = self.eye(pos) + Vec3::Y * 0.03;
        if let Some(t) = segment_sphere(a, b, head, 0.14) {
            return Some((t, true));
        }
        let fwd = forward_of_yaw(self.yaw);
        let (p0, p1, r) = match self.stance {
            Stance::Stand => (pos + Vec3::Y * 0.25, pos + Vec3::Y * 1.42, 0.27),
            Stance::Crouch => (pos + Vec3::Y * 0.25, pos + Vec3::Y * 0.92, 0.3),
            Stance::Prone => (
                pos + Vec3::Y * 0.2 - fwd * 0.8,
                pos + Vec3::Y * 0.22 + fwd * 0.55,
                0.22,
            ),
        };
        let (s, _, d2) = closest_segments(a, b, p0, p1);
        (d2 < r * r).then_some((s, false))
    }

    pub fn capsule_height(&self) -> f32 {
        match self.stance {
            Stance::Stand => 1.8,
            Stance::Crouch => 1.2,
            Stance::Prone => 0.5,
        }
    }
}

#[derive(Component)]
pub struct Loadout {
    pub weapons: Vec<Weapon>,
    pub active: usize,
    pub grenades: u32,
    pub switching: f32,
    pub throw_cooldown: f32,
}

impl Loadout {
    pub fn for_role(faction: Faction, role: Role) -> Self {
        let rifle = match faction {
            Faction::Blufor => WeaponKind::Rifle,
            Faction::Opfor => WeaponKind::Ak,
        };
        let mg = match faction {
            Faction::Blufor => WeaponKind::Lmg,
            Faction::Opfor => WeaponKind::Pkm,
        };
        let weapons = match role {
            Role::Leader => vec![Weapon::new(rifle, 7)],
            Role::Rifleman => vec![Weapon::new(rifle, 6)],
            Role::Autorifleman => vec![Weapon::new(mg, 3)],
            Role::AtSoldier => vec![Weapon::new(rifle, 4), Weapon::new(WeaponKind::Launcher, 2)],
            Role::Crew => vec![Weapon::new(rifle, 3)],
        };
        Self {
            weapons,
            active: 0,
            grenades: if role == Role::Crew { 0 } else { 2 },
            switching: 0.0,
            throw_cooldown: 0.0,
        }
    }

    pub fn current(&self) -> &Weapon {
        &self.weapons[self.active]
    }

    pub fn current_mut(&mut self) -> &mut Weapon {
        &mut self.weapons[self.active]
    }

    pub fn launcher_slot(&self) -> Option<usize> {
        self.weapons
            .iter()
            .position(|w| w.kind == WeaponKind::Launcher && (w.ammo > 0 || w.mags > 0))
    }
}

#[derive(Component)]
pub struct SoldierParts {
    pub pivot: Entity,
    pub aim: Entity,
    pub legs: [Entity; 2],
    pub rifle: Entity,
    pub launcher: Entity,
}

#[derive(Component)]
pub struct Corpse {
    pub timer: f32,
}

#[derive(Resource)]
pub struct SoldierAssets {
    leg: Handle<Mesh>,
    torso: Handle<Mesh>,
    vest: Handle<Mesh>,
    pack: Handle<Mesh>,
    head: Handle<Mesh>,
    helmet: Handle<Mesh>,
    arm: Handle<Mesh>,
    rifle: Handle<Mesh>,
    launcher: Handle<Mesh>,
    skin: Handle<StandardMaterial>,
    gun: Handle<StandardMaterial>,
    tube: Handle<StandardMaterial>,
    uniform: [Handle<StandardMaterial>; 2],
    gear: [Handle<StandardMaterial>; 2],
    helmet_mat: [Handle<StandardMaterial>; 2],
}

fn faction_index(f: Faction) -> usize {
    match f {
        Faction::Blufor => 0,
        Faction::Opfor => 1,
    }
}

pub struct SoldierPlugin;

impl Plugin for SoldierPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_soldier_assets)
            .add_systems(
                Update,
                soldier_movement
                    .in_set(Phase::Movement)
                    .run_if(is_authority),
            )
            .add_systems(
                Update,
                soldier_weapons.in_set(Phase::Combat).run_if(is_authority),
            )
            .add_systems(
                Update,
                (soldier_death, corpse_cleanup)
                    .chain()
                    .after(crate::combat::CombatPlugin::damage_label())
                    .in_set(Phase::Damage)
                    .run_if(is_authority),
            )
            .add_systems(Update, animate_soldiers.in_set(Phase::Camera));
    }
}

impl crate::combat::CombatPlugin {
    /// Damage is applied by systems in `Phase::Damage`; ordering inside the set is by chain.
    pub fn damage_label() -> impl SystemSet {
        Phase::Movement
    }
}

fn setup_soldier_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut m = |c: Color, rough: f32| {
        materials.add(StandardMaterial {
            base_color: c,
            perceptual_roughness: rough,
            ..default()
        })
    };
    commands.insert_resource(SoldierAssets {
        leg: meshes.add(Cuboid::new(0.17, 0.88, 0.2)),
        torso: meshes.add(Cuboid::new(0.42, 0.6, 0.24)),
        vest: meshes.add(Cuboid::new(0.46, 0.42, 0.3)),
        pack: meshes.add(Cuboid::new(0.34, 0.42, 0.18)),
        head: meshes.add(Sphere::new(0.11)),
        helmet: meshes.add(Sphere::new(0.135)),
        arm: meshes.add(Cuboid::new(0.1, 0.5, 0.1)),
        rifle: meshes.add(Cuboid::new(0.05, 0.1, 0.85)),
        launcher: meshes.add(Cylinder::new(0.065, 1.0)),
        skin: m(Color::srgb(0.75, 0.58, 0.45), 0.8),
        gun: m(Color::srgb(0.1, 0.1, 0.1), 0.5),
        tube: m(Color::srgb(0.3, 0.33, 0.22), 0.7),
        uniform: [
            m(Color::srgb(0.52, 0.46, 0.35), 0.95),
            m(Color::srgb(0.27, 0.31, 0.22), 0.95),
        ],
        gear: [
            m(Color::srgb(0.42, 0.38, 0.28), 0.9),
            m(Color::srgb(0.16, 0.18, 0.14), 0.9),
        ],
        helmet_mat: [
            m(Color::srgb(0.47, 0.43, 0.32), 0.8),
            m(Color::srgb(0.24, 0.27, 0.2), 0.8),
        ],
    });
}

const BLUFOR_NAMES: &[&str] = &[
    "Miller", "Novak", "Reyes", "Okafor", "Lindqvist", "Brennan", "Kowalski", "Hayes", "Tanaka",
    "Dubois", "Fischer", "Walsh", "Morales", "Price", "Sato", "Grant", "Iversen", "Shaw",
];
const OPFOR_NAMES: &[&str] = &[
    "Ivanov", "Petrov", "Sokolov", "Volkov", "Morozov", "Kuznetsov", "Popov", "Lebedev",
    "Orlov", "Zaytsev", "Belov", "Gusev", "Titov", "Fedorov", "Kozlov", "Nikitin",
];

pub fn random_name(faction: Faction, role: Role) -> String {
    let rank = match role {
        Role::Leader => "Sgt.",
        Role::Autorifleman | Role::AtSoldier => "Cpl.",
        _ => "Pvt.",
    };
    let list = match faction {
        Faction::Blufor => BLUFOR_NAMES,
        Faction::Opfor => OPFOR_NAMES,
    };
    format!("{rank} {}", list[fastrand::usize(..list.len())])
}

/// Spawn the full soldier (model + components). The caller adds AI or `Human`.
pub fn spawn_soldier(
    commands: &mut Commands,
    assets: &SoldierAssets,
    name: String,
    faction: Faction,
    role: Role,
    pos: Vec3,
    yaw: f32,
) -> Entity {
    let fi = faction_index(faction);
    let part = |mesh: &Handle<Mesh>, mat: &Handle<StandardMaterial>, t: Transform| {
        (Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), t)
    };
    let root = commands
        .spawn((
            Soldier::new(name, faction, role, yaw),
            Loadout::for_role(faction, role),
            Health::new(100.0, 0),
            Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
            Visibility::default(),
            MissionEntity,
        ))
        .id();
    let legs = [-0.11f32, 0.11].map(|x| {
        commands
            .spawn((Transform::from_xyz(x, 0.9, 0.0), Visibility::default()))
            .with_child(part(
                &assets.leg,
                &assets.uniform[fi],
                Transform::from_xyz(0.0, -0.44, 0.0),
            ))
            .id()
    });
    let rifle = commands
        .spawn(part(
            &assets.rifle,
            &assets.gun,
            Transform::from_xyz(0.13, -0.05, -0.4),
        ))
        .id();
    let launcher = commands
        .spawn((
            part(
                &assets.launcher,
                &assets.tube,
                Transform::from_xyz(0.17, 0.08, -0.15)
                    .with_rotation(Quat::from_rotation_x(FRAC_PI_2)),
            ),
            if role == Role::AtSoldier {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            },
        ))
        .id();
    let aim = commands
        .spawn((Transform::from_xyz(0.0, 1.42, 0.0), Visibility::default()))
        .with_children(|p| {
            for x in [-0.2f32, 0.2] {
                p.spawn(part(
                    &assets.arm,
                    &assets.uniform[fi],
                    Transform::from_xyz(x * 0.6, -0.05, -0.2)
                        .with_rotation(Quat::from_rotation_x(FRAC_PI_2) * Quat::from_rotation_z(x)),
                ));
            }
        })
        .add_children(&[rifle, launcher])
        .id();
    let pivot = commands
        .spawn((Transform::default(), Visibility::default()))
        .with_children(|p| {
            p.spawn(part(
                &assets.torso,
                &assets.uniform[fi],
                Transform::from_xyz(0.0, 1.22, 0.0),
            ));
            p.spawn(part(
                &assets.vest,
                &assets.gear[fi],
                Transform::from_xyz(0.0, 1.25, 0.0),
            ));
            p.spawn(part(
                &assets.pack,
                &assets.gear[fi],
                Transform::from_xyz(0.0, 1.25, 0.22),
            ));
            p.spawn(part(
                &assets.head,
                &assets.skin,
                Transform::from_xyz(0.0, 1.62, 0.0),
            ));
            p.spawn(part(
                &assets.helmet,
                &assets.helmet_mat[fi],
                Transform::from_xyz(0.0, 1.67, 0.01).with_scale(Vec3::new(1.0, 0.75, 1.05)),
            ));
        })
        .add_children(&legs)
        .add_child(aim)
        .id();
    commands.entity(root).add_child(pivot).insert(SoldierParts {
        pivot,
        aim,
        legs,
        rifle,
        launcher,
    });
    root
}

/// Soldiers keep out of vehicles' hulls.
fn vehicle_obstacle(tf: &Transform, v: &Vehicle) -> Obstacle {
    let half = v.spec().half;
    let (yaw, _, _) = tf.rotation.to_euler(EulerRot::YXZ);
    Obstacle {
        pos: tf.translation + Vec3::Y * half.y,
        shape: Shape::Obb { half, yaw },
        mask: SOLID,
        entity: None,
        active: true,
        tree: false,
    }
}

#[allow(clippy::type_complexity)]
fn soldier_movement(
    time: Res<Time>,
    terrain: Res<Terrain>,
    obstacles: Res<Obstacles>,
    mut soldiers: Query<(&mut Soldier, &mut Transform, &Health, &Loadout)>,
    vehicles: Query<(&Transform, &Vehicle), Without<Soldier>>,
) {
    let dt = time.delta_secs().min(0.05);
    let vehicle_obs: Vec<Obstacle> = vehicles
        .iter()
        .filter(|(_, v)| v.spec().half.y < 3.0 || !v.airborne)
        .map(|(tf, v)| vehicle_obstacle(tf, v))
        .collect();
    for (mut s, mut tf, health, loadout) in &mut soldiers {
        s.suppression = (s.suppression - dt * 0.2).max(0.0);
        s.since_shot += dt;
        if health.dead || s.vehicle.is_some() {
            s.velocity = Vec3::ZERO;
            continue;
        }
        if let Some(st) = s.intent.stance.take() {
            s.stance = st;
        }
        let launcher_out = loadout.current().kind == WeaponKind::Launcher;
        let mut dir = s.intent.move_dir;
        dir.y = 0.0;
        if dir.length_squared() > 1.0 {
            dir = dir.normalize();
        }
        let moving = dir.length_squared() > 0.01;
        let forwardish = dir.dot(forward_of_yaw(s.yaw)) > 0.5;
        let can_sprint = s.intent.sprint && s.stamina > 0.05 && !s.intent.ads && forwardish;
        if can_sprint && s.stance != Stance::Stand {
            s.stance = Stance::Stand;
        }
        s.sprinting = can_sprint && moving;
        let mut speed = match s.stance {
            Stance::Stand if s.sprinting => 6.2,
            Stance::Stand => 3.4,
            Stance::Crouch => 2.1,
            Stance::Prone => 0.75,
        };
        if s.intent.ads || launcher_out {
            speed *= 0.55;
        }
        if s.healing > 0.0 {
            speed = 0.0;
        }
        speed *= 0.6 + 0.4 * s.stamina.max(0.3);
        let desired = dir * speed;
        let k = (dt * 9.0).min(1.0);
        let v = s.velocity;
        s.velocity = v + (desired - v) * k;
        if s.sprinting {
            s.stamina = (s.stamina - dt * 0.07).max(0.0);
        } else {
            s.stamina = (s.stamina + dt * if moving { 0.05 } else { 0.1 }).min(1.0);
        }

        let pos = tf.translation;
        let mut new = pos + s.velocity * dt;
        // Steep uphill slows to a crawl.
        let n = terrain.normal(new.x, new.z);
        let uphill = -Vec2::new(n.x, n.z).dot(Vec2::new(s.velocity.x, s.velocity.z));
        if n.y < 0.72 && uphill > 0.0 {
            new = pos + s.velocity * dt * 0.25;
        }
        // Deep water is impassable.
        if terrain.height(new.x, new.z) < -1.3 {
            new = pos;
            s.velocity = Vec3::ZERO;
        }
        new.y = terrain.height(new.x, new.z);
        let h = s.capsule_height();
        new = obstacles.push_out(new, 0.32, h);
        for o in &vehicle_obs {
            if let Some(push) = o.push(new, 0.35, h) {
                new.x += push.x;
                new.z += push.y;
            }
        }
        new.x = new.x.clamp(-MAP_HALF + 5.0, MAP_HALF - 5.0);
        new.z = new.z.clamp(-MAP_HALF + 5.0, MAP_HALF - 5.0);
        new.y = terrain.height(new.x, new.z);
        let speed_now = Vec2::new(new.x - pos.x, new.z - pos.z).length() / dt.max(1e-4);
        s.walk_phase += speed_now * dt * 2.2;
        tf.translation = new;
        tf.rotation = Quat::from_rotation_y(s.yaw);
        s.pitch = s.pitch.clamp(-1.45, 1.45);
    }
}

#[allow(clippy::type_complexity)]
fn soldier_weapons(
    time: Res<Time>,
    mut soldiers: Query<(Entity, &mut Soldier, &mut Loadout, &mut Health, &Transform)>,
    mut shooter: Shooter,
) {
    let dt = time.delta_secs();
    for (e, mut s, mut loadout, mut health, tf) in &mut soldiers {
        for w in loadout.weapons.iter_mut() {
            w.tick(dt);
        }
        loadout.switching -= dt;
        loadout.throw_cooldown -= dt;
        let intent = std::mem::take(&mut s.intent);
        // Movement intent persists until the controller overwrites it next frame.
        s.intent.move_dir = intent.move_dir;
        s.intent.sprint = intent.sprint;
        s.intent.ads = intent.ads;
        s.held = loadout.current().kind;
        if health.dead || s.vehicle.is_some() {
            continue;
        }
        if s.healing > 0.0 {
            s.healing -= dt;
            if s.healing <= 0.0 {
                s.healing = 0.0;
                health.hp = health.hp.max(health.max * 0.9);
            }
            continue;
        }
        if intent.heal && s.fak > 0 && health.hp < health.max && !s.sprinting {
            s.fak -= 1;
            s.healing = 5.0;
            shooter
                .sounds
                .write(PlaySound::new(SoundKind::Reload, tf.translation));
            continue;
        }
        if let Some(slot) = intent.switch_to
            && slot < loadout.weapons.len()
            && slot != loadout.active
        {
            loadout.active = slot;
            loadout.switching = if loadout.current().kind == WeaponKind::Launcher {
                1.6
            } else {
                1.0
            };
            s.held = loadout.current().kind;
        }
        if intent.toggle_mode {
            let w = loadout.current_mut();
            if w.stats().full_auto {
                w.auto_mode = !w.auto_mode;
            }
        }
        if intent.reload && loadout.current_mut().start_reload() {
            shooter
                .sounds
                .write(PlaySound::new(SoundKind::Reload, tf.translation));
        }
        let eye = s.eye(tf.translation);
        if intent.throw && loadout.grenades > 0 && loadout.throw_cooldown <= 0.0 && !s.sprinting {
            loadout.grenades -= 1;
            loadout.throw_cooldown = 1.5;
            let dir = dir_from_angles(s.yaw, (s.pitch + 0.25).min(1.2));
            let vel = dir * crate::combat::GRENADE.muzzle_velocity + s.velocity;
            spawn_projectile(
                &mut shooter.commands,
                &shooter.fx,
                WeaponKind::Grenade,
                eye + dir * 0.5,
                vel,
                Some(e),
                None,
                Some(s.faction),
                false,
                false,
            );
            shooter.shots.write(crate::combat::ShotFx {
                origin: eye + dir * 0.5,
                vel,
                kind: WeaponKind::Grenade,
                tracer: false,
            });
            shooter
                .sounds
                .write(PlaySound::new(SoundKind::Throw, tf.translation));
            continue;
        }
        if !intent.fire {
            s.dry_clicked = false;
            continue;
        }
        if loadout.switching > 0.0 || s.sprinting {
            continue;
        }
        let moving = s.velocity.length() > 0.5;
        let stance_k = match s.stance {
            Stance::Stand => 1.0,
            Stance::Crouch => 0.75,
            Stance::Prone => 0.5,
        };
        let w = loadout.current_mut();
        let stats = w.stats();
        if w.ammo == 0 {
            if !w.start_reload() && !s.dry_clicked {
                s.dry_clicked = true;
                shooter
                    .sounds
                    .write(PlaySound::new(SoundKind::DryFire, tf.translation).local());
            } else if w.reloading > 0.0 && w.reloading == stats.reload_time {
                shooter
                    .sounds
                    .write(PlaySound::new(SoundKind::Reload, tf.translation));
            }
            continue;
        }
        if !w.try_fire() {
            continue;
        }
        let tracer = w.is_tracer();
        let kind = w.kind;
        let mut spread = stats.spread * stance_k;
        if intent.ads {
            spread *= stats.ads_factor;
        }
        if moving {
            spread *= 2.0;
        }
        spread *= 1.0 + s.suppression;
        let dir = apply_spread(s.aim_dir(), spread);
        s.since_shot = 0.0;
        shooter.fire(
            kind,
            eye + dir * 0.35,
            dir,
            Vec3::ZERO,
            e,
            None,
            s.faction,
            tracer,
        );
        if loadout.current().ammo == 0 && loadout.current_mut().start_reload() {
            shooter
                .sounds
                .write(PlaySound::new(SoundKind::Reload, tf.translation));
        }
    }
}

fn soldier_death(
    mut commands: Commands,
    mut events: MessageReader<Killed>,
    mut soldiers: Query<(&mut Soldier, &Transform)>,
    mut vehicles: Query<&mut Vehicle>,
    names: Query<&Soldier>,
    mut radio: MessageWriter<Radio>,
) {
    for ev in events.read() {
        let killer = ev
            .attacker
            .filter(|&a| a != ev.victim)
            .and_then(|a| names.get(a).ok())
            .map(|s| (s.name.clone(), s.faction));
        let Ok((mut s, _)) = soldiers.get_mut(ev.victim) else {
            continue;
        };
        if let Some(v) = s.vehicle.take()
            && let Ok(mut veh) = vehicles.get_mut(v)
        {
            veh.remove_occupant(ev.victim);
        }
        s.velocity = Vec3::ZERO;
        s.healing = 0.0;
        let text = match &killer {
            Some((k, kf)) if *kf == s.faction => format!("{} was killed by {} (friendly fire)", s.name, k),
            Some((k, _)) => format!("{} killed {}", k, s.name),
            None => format!("{} was killed", s.name),
        };
        let color = if s.faction == Faction::Blufor {
            RADIO_KILL
        } else {
            Color::srgb(0.85, 0.85, 0.85)
        };
        radio.write(Radio::new(text, color));
        commands.entity(ev.victim).insert(Corpse { timer: 150.0 });
    }
}

fn corpse_cleanup(
    mut commands: Commands,
    time: Res<Time>,
    mut corpses: Query<(Entity, &mut Corpse)>,
) {
    for (e, mut c) in &mut corpses {
        c.timer -= time.delta_secs();
        if c.timer <= 0.0 {
            commands.entity(e).despawn();
        }
    }
}

#[allow(clippy::type_complexity)]
fn animate_soldiers(
    time: Res<Time>,
    soldiers: Query<(&Soldier, &Health, &SoldierParts, &Transform)>,
    mut parts: Query<(&mut Transform, &mut Visibility), Without<Soldier>>,
) {
    let dt = time.delta_secs();
    let k = (dt * 10.0).min(1.0);
    for (s, health, sp, _) in &soldiers {
        let (target_t, target_r) = if health.dead {
            (Vec3::new(0.0, 0.12, 0.0), Quat::from_rotation_x(FRAC_PI_2))
        } else {
            match s.stance {
                Stance::Stand => (Vec3::ZERO, Quat::IDENTITY),
                Stance::Crouch => (Vec3::new(0.0, -0.42, 0.0), Quat::from_rotation_x(-0.2)),
                Stance::Prone => (Vec3::new(0.0, 0.2, 0.85), Quat::from_rotation_x(-FRAC_PI_2)),
            }
        };
        if let Ok((mut t, _)) = parts.get_mut(sp.pivot) {
            t.translation = t.translation.lerp(target_t, k);
            t.rotation = t.rotation.slerp(target_r, k);
        }
        let swing = if health.dead {
            0.0
        } else {
            s.walk_phase.sin() * (s.velocity.length() / 4.0).min(1.0) * 0.6
        };
        let crouch = if s.stance == Stance::Crouch && !health.dead {
            -1.1
        } else {
            0.0
        };
        for (i, leg) in sp.legs.iter().enumerate() {
            if let Ok((mut t, _)) = parts.get_mut(*leg) {
                let sign = if i == 0 { 1.0 } else { -1.0 };
                let target = Quat::from_rotation_x(swing * sign + crouch);
                t.rotation = t.rotation.slerp(target, k);
            }
        }
        if let Ok((mut t, _)) = parts.get_mut(sp.aim) {
            let pitch = match (health.dead, s.stance) {
                (true, _) => 0.0,
                (_, Stance::Prone) => s.pitch * 0.5 + FRAC_PI_2 * 0.95,
                (_, Stance::Crouch) => s.pitch + 0.2,
                _ if s.sprinting => -0.6,
                _ => s.pitch,
            };
            t.rotation = Quat::from_rotation_x(pitch);
        }
        let launcher = s.held == WeaponKind::Launcher;
        if let Ok((_, mut v)) = parts.get_mut(sp.rifle) {
            *v = if launcher {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            };
        }
        if let Ok((mut t, mut v)) = parts.get_mut(sp.launcher) {
            *v = if launcher || s.role == Role::AtSoldier {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
            // Carried on the back unless in use.
            *t = if launcher {
                Transform::from_xyz(0.17, 0.08, -0.15)
                    .with_rotation(Quat::from_rotation_x(FRAC_PI_2))
            } else {
                Transform::from_xyz(0.1, -0.1, 0.35)
                    .with_rotation(Quat::from_rotation_x(FRAC_PI_2 * 0.5))
            };
        }
    }
}

/// Muzzle flash / shadow casters for the local first-person body are hidden elsewhere.
#[allow(dead_code)]
pub fn hide_shadow(commands: &mut Commands, e: Entity) {
    commands.entity(e).insert(NotShadowCaster);
}
