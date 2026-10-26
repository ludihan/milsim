//! Weapons, ballistics, explosions, damage and visual effects.

use bevy::{ecs::system::SystemParam, light::NotShadowCaster, prelude::*};

use crate::audio::{PlaySound, SoundKind};
use crate::common::*;
use crate::net::NetMode;
use crate::player::{Human, Humans};
use crate::soldier::{Soldier, Stance};
use crate::terrain::Terrain;
use crate::vehicle::Vehicle;
use crate::world::{BULLET, Obstacles, TreeFell};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[repr(u8)]
pub enum WeaponKind {
    Rifle,
    Ak,
    Lmg,
    Pkm,
    Launcher,
    Hmg,
    Cannon,
    Coax,
    Minigun,
    Rockets,
    Grenade,
}

impl WeaponKind {
    pub const ALL: [WeaponKind; 11] = [
        WeaponKind::Rifle,
        WeaponKind::Ak,
        WeaponKind::Lmg,
        WeaponKind::Pkm,
        WeaponKind::Launcher,
        WeaponKind::Hmg,
        WeaponKind::Cannon,
        WeaponKind::Coax,
        WeaponKind::Minigun,
        WeaponKind::Rockets,
        WeaponKind::Grenade,
    ];

    pub fn from_u8(v: u8) -> Self {
        Self::ALL.get(v as usize).copied().unwrap_or(WeaponKind::Rifle)
    }

    pub fn stats(self) -> &'static WeaponStats {
        match self {
            WeaponKind::Rifle => &RIFLE,
            WeaponKind::Ak => &AK,
            WeaponKind::Lmg => &LMG,
            WeaponKind::Pkm => &PKM,
            WeaponKind::Launcher => &LAUNCHER,
            WeaponKind::Hmg => &HMG,
            WeaponKind::Cannon => &CANNON,
            WeaponKind::Coax => &COAX,
            WeaponKind::Minigun => &MINIGUN,
            WeaponKind::Rockets => &ROCKETS,
            WeaponKind::Grenade => &GRENADE,
        }
    }

    pub fn is_explosive(self) -> bool {
        self.stats().blast_radius > 0.0
    }
}

pub struct WeaponStats {
    pub name: &'static str,
    pub muzzle_velocity: f32,
    pub damage: f32,
    pub ap: u8,
    pub interval: f32,
    pub mag_size: u32,
    pub reload_time: f32,
    /// Cone half-angle in radians when firing from the hip.
    pub spread: f32,
    /// Fraction of `spread` left when aiming down sights.
    pub ads_factor: f32,
    pub recoil: f32,
    pub gravity: f32,
    pub drag: f32,
    pub blast_radius: f32,
    pub blast_damage: f32,
    pub tracer_every: u32,
    pub full_auto: bool,
    pub sound: SoundKind,
    /// How far away enemies hear it.
    pub loudness: f32,
}

const fn bullet(
    name: &'static str,
    muzzle_velocity: f32,
    damage: f32,
    interval: f32,
    mag_size: u32,
    reload_time: f32,
    spread: f32,
    recoil: f32,
    tracer_every: u32,
    sound: SoundKind,
) -> WeaponStats {
    WeaponStats {
        name,
        muzzle_velocity,
        damage,
        ap: 1,
        interval,
        mag_size,
        reload_time,
        spread,
        ads_factor: 0.12,
        recoil,
        gravity: 1.0,
        drag: 0.35,
        blast_radius: 0.0,
        blast_damage: 0.0,
        tracer_every,
        full_auto: true,
        sound,
        loudness: 350.0,
    }
}

pub static RIFLE: WeaponStats = bullet(
    "M4A1 5.56mm",
    880.0,
    36.0,
    0.075,
    30,
    2.4,
    0.022,
    0.007,
    0,
    SoundKind::RifleShot,
);
pub static AK: WeaponStats = bullet(
    "AKM 7.62mm",
    715.0,
    42.0,
    0.1,
    30,
    2.6,
    0.026,
    0.01,
    0,
    SoundKind::AkShot,
);
pub static LMG: WeaponStats = WeaponStats {
    tracer_every: 4,
    ..bullet(
        "M249 SAW",
        900.0,
        34.0,
        0.07,
        100,
        5.5,
        0.035,
        0.006,
        4,
        SoundKind::MgShot,
    )
};
pub static PKM: WeaponStats = WeaponStats {
    ..bullet(
        "PKM 7.62mm",
        825.0,
        44.0,
        0.09,
        100,
        6.0,
        0.035,
        0.008,
        4,
        SoundKind::MgShot,
    )
};
pub static LAUNCHER: WeaponStats = WeaponStats {
    name: "AT-4 Launcher",
    muzzle_velocity: 170.0,
    damage: 520.0,
    ap: 3,
    interval: 1.0,
    mag_size: 1,
    reload_time: 4.0,
    spread: 0.03,
    ads_factor: 0.08,
    recoil: 0.04,
    gravity: 0.35,
    drag: 0.0,
    blast_radius: 5.0,
    blast_damage: 130.0,
    tracer_every: 0,
    full_auto: false,
    sound: SoundKind::LauncherShot,
    loudness: 500.0,
};
pub static HMG: WeaponStats = WeaponStats {
    ap: 2,
    ads_factor: 0.4,
    loudness: 700.0,
    ..bullet(
        "M2 .50 cal",
        890.0,
        75.0,
        0.11,
        100,
        6.0,
        0.008,
        0.004,
        2,
        SoundKind::HmgShot,
    )
};
pub static CANNON: WeaponStats = WeaponStats {
    name: "30mm HE Cannon",
    muzzle_velocity: 950.0,
    damage: 450.0,
    ap: 3,
    interval: 0.35,
    mag_size: 10,
    reload_time: 5.0,
    spread: 0.003,
    ads_factor: 0.5,
    recoil: 0.01,
    gravity: 1.0,
    drag: 0.1,
    blast_radius: 4.5,
    blast_damage: 110.0,
    tracer_every: 1,
    full_auto: false,
    sound: SoundKind::CannonShot,
    loudness: 900.0,
};
pub static COAX: WeaponStats = WeaponStats {
    ads_factor: 0.4,
    ..bullet(
        "Coax MG 7.62mm",
        850.0,
        42.0,
        0.09,
        200,
        6.0,
        0.008,
        0.002,
        3,
        SoundKind::MgShot,
    )
};
pub static MINIGUN: WeaponStats = WeaponStats {
    ap: 2,
    ads_factor: 0.6,
    loudness: 800.0,
    ..bullet(
        "M134 Minigun",
        900.0,
        36.0,
        0.03,
        600,
        8.0,
        0.012,
        0.0,
        3,
        SoundKind::MgShot,
    )
};
pub static ROCKETS: WeaponStats = WeaponStats {
    name: "Hydra Rockets",
    muzzle_velocity: 320.0,
    damage: 380.0,
    ap: 3,
    interval: 0.22,
    mag_size: 14,
    reload_time: 20.0,
    spread: 0.012,
    ads_factor: 1.0,
    recoil: 0.0,
    gravity: 0.25,
    drag: 0.0,
    blast_radius: 6.0,
    blast_damage: 160.0,
    tracer_every: 0,
    full_auto: true,
    sound: SoundKind::LauncherShot,
    loudness: 700.0,
};
pub static GRENADE: WeaponStats = WeaponStats {
    name: "M67 Grenade",
    muzzle_velocity: 17.0,
    damage: 0.0,
    ap: 1,
    interval: 1.2,
    mag_size: 1,
    reload_time: 0.0,
    spread: 0.0,
    ads_factor: 1.0,
    recoil: 0.0,
    gravity: 1.0,
    drag: 0.0,
    blast_radius: 9.0,
    blast_damage: 170.0,
    tracer_every: 0,
    full_auto: false,
    sound: SoundKind::Throw,
    loudness: 0.0,
};

#[derive(Clone, Debug)]
pub struct Weapon {
    pub kind: WeaponKind,
    pub ammo: u32,
    pub mags: u32,
    pub cooldown: f32,
    pub reloading: f32,
    pub shots: u32,
    pub auto_mode: bool,
}

impl Weapon {
    pub fn new(kind: WeaponKind, mags: u32) -> Self {
        Self {
            kind,
            ammo: kind.stats().mag_size,
            mags,
            cooldown: 0.0,
            reloading: 0.0,
            shots: 0,
            auto_mode: kind.stats().full_auto,
        }
    }

    pub fn stats(&self) -> &'static WeaponStats {
        self.kind.stats()
    }

    pub fn tick(&mut self, dt: f32) {
        self.cooldown -= dt;
        if self.reloading > 0.0 {
            self.reloading -= dt;
            if self.reloading <= 0.0 {
                self.reloading = 0.0;
                self.ammo = self.stats().mag_size;
                self.mags = self.mags.saturating_sub(1);
            }
        }
    }

    pub fn start_reload(&mut self) -> bool {
        if self.reloading <= 0.0 && self.mags > 0 && self.ammo < self.stats().mag_size {
            self.reloading = self.stats().reload_time;
            true
        } else {
            false
        }
    }

    pub fn try_fire(&mut self) -> bool {
        if self.cooldown <= 0.0 && self.reloading <= 0.0 && self.ammo > 0 {
            self.ammo -= 1;
            self.cooldown = self.stats().interval;
            self.shots += 1;
            true
        } else {
            false
        }
    }

    pub fn is_tracer(&self) -> bool {
        let every = self.stats().tracer_every;
        every > 0 && self.shots % every == 0
    }
}

/// Randomly deflect `dir` inside a cone of half-angle `spread`.
pub fn apply_spread(dir: Vec3, spread: f32) -> Vec3 {
    if spread <= 0.0 {
        return dir;
    }
    let up = if dir.y.abs() > 0.95 { Vec3::X } else { Vec3::Y };
    let right = dir.cross(up).normalize();
    let up = right.cross(dir).normalize();
    let c = rand_in_circle() * spread;
    (dir + right * c.x + up * c.y).normalize()
}

#[derive(Component)]
pub struct Projectile {
    pub kind: WeaponKind,
    pub vel: Vec3,
    pub shooter: Option<Entity>,
    pub ignore: Option<Entity>,
    pub faction: Option<Faction>,
    pub life: f32,
    pub tracer: bool,
    /// Client-side copy that only draws; the host decides what it hits.
    pub visual: bool,
    pub age: f32,
    pub cracked: bool,
}

/// A shot was fired (drives muzzle flashes and client-side projectiles).
#[derive(Message, Clone, Copy)]
pub struct ShotFx {
    pub origin: Vec3,
    pub vel: Vec3,
    pub kind: WeaponKind,
    pub tracer: bool,
}

#[derive(Message, Clone, Copy)]
pub struct ExplosionFx {
    pub pos: Vec3,
    pub size: f32,
}

/// Authoritative explosion that deals damage.
#[derive(Message, Clone, Copy)]
pub struct Explosion {
    pub pos: Vec3,
    pub kind: WeaponKind,
    pub attacker: Option<Entity>,
}

/// AI can hear this shot.
#[derive(Message, Clone, Copy)]
pub struct Gunshot {
    pub pos: Vec3,
    pub faction: Faction,
    pub shooter: Entity,
    pub loudness: f32,
}

#[derive(Message, Clone, Copy)]
pub struct Suppress {
    pub target: Entity,
    pub amount: f32,
    pub from: Vec3,
}

#[derive(Resource)]
pub struct FxAssets {
    pub sphere: Handle<Mesh>,
    pub rocket: Handle<Mesh>,
    pub flash: Handle<StandardMaterial>,
    pub smoke_dark: Handle<StandardMaterial>,
    pub smoke_light: Handle<StandardMaterial>,
    pub dirt: Handle<StandardMaterial>,
    pub blood: Handle<StandardMaterial>,
    pub spark: Handle<StandardMaterial>,
    pub fire: Handle<StandardMaterial>,
    pub rocket_mat: Handle<StandardMaterial>,
    pub grenade_mat: Handle<StandardMaterial>,
}

#[derive(Component)]
pub struct Fx {
    pub life: f32,
    pub max: f32,
    pub grow: f32,
    pub vel: Vec3,
    pub base_scale: f32,
}

/// Everything needed to fire a weapon from a system.
#[derive(SystemParam)]
pub struct Shooter<'w, 's> {
    pub commands: Commands<'w, 's>,
    pub fx: Res<'w, FxAssets>,
    pub shots: MessageWriter<'w, ShotFx>,
    pub sounds: MessageWriter<'w, PlaySound>,
    pub gunshots: MessageWriter<'w, Gunshot>,
}

impl Shooter<'_, '_> {
    #[allow(clippy::too_many_arguments)]
    pub fn fire(
        &mut self,
        kind: WeaponKind,
        origin: Vec3,
        dir: Vec3,
        inherit_vel: Vec3,
        shooter: Entity,
        ignore: Option<Entity>,
        faction: Faction,
        tracer: bool,
    ) {
        let stats = kind.stats();
        let vel = dir * stats.muzzle_velocity + inherit_vel;
        spawn_projectile(
            &mut self.commands,
            &self.fx,
            kind,
            origin,
            vel,
            Some(shooter),
            ignore,
            Some(faction),
            tracer,
            false,
        );
        self.shots.write(ShotFx {
            origin,
            vel,
            kind,
            tracer,
        });
        self.sounds.write(PlaySound::new(stats.sound, origin));
        if stats.loudness > 0.0 {
            self.gunshots.write(Gunshot {
                pos: origin,
                faction,
                shooter,
                loudness: stats.loudness,
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_projectile(
    commands: &mut Commands,
    fx: &FxAssets,
    kind: WeaponKind,
    origin: Vec3,
    vel: Vec3,
    shooter: Option<Entity>,
    ignore: Option<Entity>,
    faction: Option<Faction>,
    tracer: bool,
    visual: bool,
) {
    let life = match kind {
        WeaponKind::Grenade => 4.0,
        WeaponKind::Launcher | WeaponKind::Rockets => 6.0,
        _ => 3.0,
    };
    let mut e = commands.spawn((
        Projectile {
            kind,
            vel,
            shooter,
            ignore,
            faction,
            life,
            tracer,
            visual,
            age: 0.0,
            cracked: false,
        },
        Transform::from_translation(origin).looking_to(vel.normalize_or(Vec3::NEG_Z), Vec3::Y),
        Visibility::default(),
        MissionEntity,
    ));
    match kind {
        WeaponKind::Launcher | WeaponKind::Rockets => {
            e.with_child((
                Mesh3d(fx.rocket.clone()),
                MeshMaterial3d(fx.rocket_mat.clone()),
                Transform::from_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                NotShadowCaster,
            ));
        }
        WeaponKind::Grenade => {
            e.with_child((
                Mesh3d(fx.sphere.clone()),
                MeshMaterial3d(fx.grenade_mat.clone()),
                Transform::from_scale(Vec3::splat(0.05)),
                NotShadowCaster,
            ));
        }
        _ => {}
    }
}

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ShotFx>()
            .add_message::<ExplosionFx>()
            .add_message::<Explosion>()
            .add_message::<Gunshot>()
            .add_message::<Suppress>()
            .add_message::<Damage>()
            .add_message::<Killed>()
            .add_systems(Startup, setup_fx_assets)
            .add_systems(
                Update,
                (
                    update_projectiles,
                    (handle_explosions, apply_suppression, apply_damage)
                        .chain()
                        .run_if(crate::net::is_authority),
                )
                    .chain()
                    .in_set(Phase::Damage),
            )
            .add_systems(
                Update,
                (spawn_shot_fx, spawn_explosion_fx, update_fx).in_set(Phase::Camera),
            );
    }
}

fn setup_fx_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let unlit = |materials: &mut Assets<StandardMaterial>, c: Color, emissive: LinearRgba| {
        materials.add(StandardMaterial {
            base_color: c,
            emissive,
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            ..default()
        })
    };
    let smoke = |materials: &mut Assets<StandardMaterial>, c: Color| {
        materials.add(StandardMaterial {
            base_color: c,
            perceptual_roughness: 1.0,
            alpha_mode: AlphaMode::Blend,
            ..default()
        })
    };
    commands.insert_resource(FxAssets {
        sphere: meshes.add(Sphere::new(1.0).mesh().ico(1).unwrap()),
        rocket: meshes.add(Cylinder::new(0.045, 0.9)),
        flash: unlit(
            &mut materials,
            Color::srgba(1.0, 0.75, 0.3, 0.9),
            LinearRgba::rgb(30.0, 14.0, 3.0),
        ),
        fire: unlit(
            &mut materials,
            Color::srgba(1.0, 0.45, 0.1, 0.85),
            LinearRgba::rgb(12.0, 4.0, 0.5),
        ),
        spark: unlit(
            &mut materials,
            Color::srgba(1.0, 0.9, 0.6, 1.0),
            LinearRgba::rgb(20.0, 15.0, 6.0),
        ),
        smoke_dark: smoke(&mut materials, Color::srgba(0.12, 0.12, 0.12, 0.6)),
        smoke_light: smoke(&mut materials, Color::srgba(0.6, 0.6, 0.58, 0.45)),
        dirt: smoke(&mut materials, Color::srgba(0.42, 0.36, 0.27, 0.7)),
        blood: smoke(&mut materials, Color::srgba(0.5, 0.03, 0.02, 0.9)),
        rocket_mat: materials.add(StandardMaterial {
            base_color: Color::srgb(0.25, 0.28, 0.22),
            emissive: LinearRgba::rgb(2.0, 0.6, 0.1),
            ..default()
        }),
        grenade_mat: materials.add(Color::srgb(0.2, 0.25, 0.15)),
    });
}

pub fn spawn_puff(
    commands: &mut Commands,
    fx: &FxAssets,
    mat: &Handle<StandardMaterial>,
    pos: Vec3,
    scale: f32,
    life: f32,
    vel: Vec3,
    grow: f32,
) {
    commands.spawn((
        Mesh3d(fx.sphere.clone()),
        MeshMaterial3d(mat.clone()),
        Transform::from_translation(pos).with_scale(Vec3::splat(scale)),
        Fx {
            life,
            max: life,
            grow,
            vel,
            base_scale: scale,
        },
        NotShadowCaster,
        MissionEntity,
    ));
}

#[derive(Clone, Copy)]
enum Hit {
    Ground,
    Obstacle(usize),
    Soldier(Entity, bool),
    Vehicle(Entity),
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_projectiles(
    mut commands: Commands,
    time: Res<Time>,
    terrain: Res<Terrain>,
    obstacles: Res<Obstacles>,
    fx: Res<FxAssets>,
    mut projectiles: Query<(Entity, &mut Projectile, &mut Transform)>,
    soldiers: Query<(Entity, &Transform, &Soldier, &Health), Without<Projectile>>,
    vehicles: Query<(Entity, &Transform, &Vehicle), Without<Projectile>>,
    local: Query<(&Transform, &Soldier), (With<Player>, Without<Projectile>)>,
    mut damage: MessageWriter<Damage>,
    mut explosions: MessageWriter<Explosion>,
    mut suppress: MessageWriter<Suppress>,
    mut sounds: MessageWriter<PlaySound>,
    mut gizmos: Gizmos,
) {
    let dt = time.delta_secs();
    let local_eye = local
        .single()
        .ok()
        .filter(|(_, s)| s.vehicle.is_none())
        .map(|(t, s)| s.eye(t.translation));
    for (entity, mut p, mut tf) in &mut projectiles {
        p.age += dt;
        p.life -= dt;
        let stats = p.kind.stats();
        let a = tf.translation;
        if p.life <= 0.0 {
            if p.kind == WeaponKind::Grenade && !p.visual {
                explosions.write(Explosion {
                    pos: a,
                    kind: p.kind,
                    attacker: p.shooter,
                });
            }
            commands.entity(entity).despawn();
            continue;
        }
        p.vel.y -= GRAVITY * stats.gravity * dt;
        let drag = p.vel * stats.drag * dt;
        p.vel -= drag;
        let b = a + p.vel * dt;

        let mut best: Option<(f32, Hit)> = terrain.raycast(a, b).map(|t| (t, Hit::Ground));
        let mut consider = |t: f32, h: Hit, best: &mut Option<(f32, Hit)>| {
            if best.is_none_or(|(bt, _)| t < bt) {
                *best = Some((t, h));
            }
        };
        if let Some((t, i)) = obstacles.segment_hit(a, b, BULLET) {
            consider(t, Hit::Obstacle(i), &mut best);
        }
        let seg_mid = (a + b) * 0.5;
        let seg_r = a.distance(b) * 0.5 + 3.0;
        if p.kind != WeaponKind::Grenade || p.age > 0.15 {
            for (e, stf, s, h) in &soldiers {
                if Some(e) == p.shooter || s.vehicle.is_some() {
                    continue;
                }
                let pos = stf.translation;
                if pos.distance_squared(seg_mid) > seg_r * seg_r {
                    continue;
                }
                if !h.dead
                    && let Some(f) = p.faction
                    && f != s.faction
                    && !p.visual
                {
                    let (_, _, d2) = closest_segments(a, b, s.eye(pos), s.eye(pos));
                    if d2 < 3.5 * 3.5 {
                        suppress.write(Suppress {
                            target: e,
                            amount: 0.12 + stats.damage / 400.0,
                            from: a,
                        });
                    }
                }
                if h.dead {
                    continue;
                }
                if let Some((t, head)) = s.hit_test(pos, a, b) {
                    consider(t, Hit::Soldier(e, head), &mut best);
                }
            }
        }
        for (e, vtf, v) in &vehicles {
            if Some(e) == p.ignore {
                continue;
            }
            let half = v.spec().half;
            let center = vtf.translation + vtf.rotation * Vec3::new(0.0, half.y, 0.0);
            if center.distance_squared(seg_mid) > (seg_r + half.length()).powi(2) {
                continue;
            }
            let inv = vtf.rotation.inverse();
            if let Some(t) = segment_box_local(inv * (a - center), inv * (b - center), half) {
                consider(t, Hit::Vehicle(e), &mut best);
            }
        }

        // Supersonic crack when a round passes close to the local player.
        if let Some(eye) = local_eye
            && !p.cracked
            && p.age > 0.06
            && p.kind != WeaponKind::Grenade
        {
            let (s, _, d2) = closest_segments(a, b, eye, eye);
            if d2 < 4.0 * 4.0 && d2 > 0.3 {
                p.cracked = true;
                sounds.write(PlaySound::new(SoundKind::Crack, a.lerp(b, s)).local());
            }
        }

        if let Some((t, hit)) = best {
            let hp = a.lerp(b, t);
            if p.kind == WeaponKind::Grenade {
                let n = match hit {
                    Hit::Ground => terrain.normal(hp.x, hp.z),
                    _ => -p.vel.normalize_or(Vec3::Y),
                };
                let v = p.vel - 2.0 * p.vel.dot(n) * n;
                p.vel = v * 0.35;
                tf.translation = hp + n * 0.08;
                continue;
            }
            if stats.blast_radius > 0.0 {
                if !p.visual {
                    match hit {
                        Hit::Soldier(e, _) | Hit::Vehicle(e) => {
                            damage.write(Damage {
                                target: e,
                                amount: stats.damage,
                                ap: stats.ap,
                                attacker: p.shooter,
                                headshot: false,
                            });
                        }
                        _ => {}
                    }
                    explosions.write(Explosion {
                        pos: hp - p.vel.normalize_or_zero() * 0.3,
                        kind: p.kind,
                        attacker: p.shooter,
                    });
                }
            } else {
                match hit {
                    Hit::Ground => {
                        spawn_puff(&mut commands, &fx, &fx.dirt, hp, 0.18, 0.7, Vec3::Y * 1.2, 2.5);
                        if hp.distance_squared(local_eye.unwrap_or(Vec3::MAX)) < 900.0 {
                            sounds.write(PlaySound::new(SoundKind::Impact, hp).local());
                        }
                    }
                    Hit::Obstacle(_) | Hit::Vehicle(_) => {
                        spawn_puff(&mut commands, &fx, &fx.spark, hp, 0.07, 0.08, Vec3::ZERO, 0.0);
                        spawn_puff(
                            &mut commands,
                            &fx,
                            &fx.smoke_light,
                            hp,
                            0.12,
                            0.5,
                            Vec3::Y * 0.6,
                            2.0,
                        );
                    }
                    Hit::Soldier(..) => {
                        spawn_puff(&mut commands, &fx, &fx.blood, hp, 0.1, 0.25, Vec3::ZERO, 1.5);
                    }
                }
                if !p.visual {
                    match hit {
                        Hit::Soldier(e, head) => {
                            damage.write(Damage {
                                target: e,
                                amount: stats.damage * if head { 3.0 } else { 1.0 },
                                ap: stats.ap,
                                attacker: p.shooter,
                                headshot: head,
                            });
                        }
                        Hit::Vehicle(e) => {
                            damage.write(Damage {
                                target: e,
                                amount: stats.damage,
                                ap: stats.ap,
                                attacker: p.shooter,
                                headshot: false,
                            });
                        }
                        _ => {}
                    }
                }
            }
            commands.entity(entity).despawn();
            continue;
        }

        tf.translation = b;
        if p.vel.length_squared() > 0.01 {
            tf.look_to(p.vel.normalize(), Vec3::Y);
        }
        if stats.blast_radius == 0.0 || p.kind == WeaponKind::Cannon {
            let dir = p.vel.normalize_or_zero();
            if p.tracer {
                gizmos.line(b - dir * 7.0, b, Color::srgb(4.0, 1.4, 0.3));
            } else if p.kind != WeaponKind::Grenade {
                gizmos.line(b - dir * 2.0, b, Color::srgba(1.0, 0.95, 0.8, 0.25));
            }
        } else if p.kind != WeaponKind::Grenade && fastrand::f32() < 0.6 {
            spawn_puff(
                &mut commands,
                &fx,
                &fx.smoke_light,
                a,
                0.25,
                1.2,
                Vec3::Y * 0.3,
                1.5,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_explosions(
    mut events: MessageReader<Explosion>,
    soldiers: Query<(Entity, &Transform, &Soldier, &Health)>,
    vehicles: Query<(Entity, &Transform, &Vehicle)>,
    obstacles: Res<Obstacles>,
    mut damage: MessageWriter<Damage>,
    mut suppress: MessageWriter<Suppress>,
    mut fxs: MessageWriter<ExplosionFx>,
    mut sounds: MessageWriter<PlaySound>,
    mut falls: MessageWriter<TreeFell>,
) {
    for ex in events.read() {
        let stats = ex.kind.stats();
        let r = stats.blast_radius;
        fxs.write(ExplosionFx { pos: ex.pos, size: r });
        sounds.write(PlaySound::new(SoundKind::Explosion, ex.pos).volume(r / 6.0));
        for (e, tf, s, h) in &soldiers {
            if h.dead || s.vehicle.is_some() {
                continue;
            }
            let chest = s.chest(tf.translation);
            let d = chest.distance(ex.pos);
            if d < r * 3.0 {
                suppress.write(Suppress {
                    target: e,
                    amount: 0.6,
                    from: ex.pos,
                });
            }
            if d >= r {
                continue;
            }
            let mut f = (1.0 - d / r).powf(1.3);
            if s.stance == Stance::Prone {
                f *= 0.5;
            }
            if obstacles.segment_blocked(ex.pos + Vec3::Y * 0.2, chest, BULLET) {
                f *= 0.15;
            }
            damage.write(Damage {
                target: e,
                amount: stats.blast_damage * f + 10.0 * f,
                ap: 1,
                attacker: ex.attacker,
                headshot: false,
            });
        }
        for (e, tf, v) in &vehicles {
            let half = v.spec().half;
            let center = tf.translation + tf.rotation * Vec3::new(0.0, half.y, 0.0);
            let local = tf.rotation.inverse() * (ex.pos - center);
            let outside = (local.abs() - half).max(Vec3::ZERO).length();
            let reach = r * 0.6;
            if outside < reach {
                damage.write(Damage {
                    target: e,
                    amount: stats.blast_damage * (1.0 - outside / reach),
                    ap: stats.ap.min(2),
                    attacker: ex.attacker,
                    headshot: false,
                });
            }
        }
        if r >= 4.5 {
            obstacles.for_each_near(Vec2::new(ex.pos.x, ex.pos.z), r * 0.5, |i, o| {
                if o.tree && matches!(o.shape, crate::world::Shape::Cyl { .. }) {
                    let d = Vec2::new(o.pos.x - ex.pos.x, o.pos.z - ex.pos.z);
                    if d.length() < r * 0.5 {
                        falls.write(TreeFell {
                            obstacle: i as u32,
                            dir: d,
                        });
                    }
                }
            });
        }
    }
}

fn apply_suppression(mut events: MessageReader<Suppress>, mut soldiers: Query<&mut Soldier>) {
    for ev in events.read() {
        if let Ok(mut s) = soldiers.get_mut(ev.target) {
            s.suppression = (s.suppression + ev.amount).min(1.0);
            s.threat_from = Some(ev.from);
        }
    }
}

fn apply_damage(
    mut events: MessageReader<Damage>,
    mut targets: Query<(&mut Health, Option<&Human>)>,
    attackers: Query<&Human>,
    mut humans: ResMut<Humans>,
    mut killed: MessageWriter<Killed>,
) {
    for ev in events.read() {
        let Ok((mut health, human)) = targets.get_mut(ev.target) else {
            continue;
        };
        if health.dead {
            continue;
        }
        let amount = ev.amount * armor_factor(ev.ap, health.armor);
        if amount <= 0.0 {
            continue;
        }
        health.hp -= amount;
        if let Some(h) = human
            && let Some(slot) = humans.get_mut(h.id)
        {
            slot.hurt_count += 1;
        }
        if let Some(a) = ev.attacker
            && a != ev.target
            && let Ok(h) = attackers.get(a)
            && let Some(slot) = humans.get_mut(h.id)
        {
            slot.hit_count += 1;
        }
        if health.hp <= 0.0 {
            health.hp = 0.0;
            health.dead = true;
            killed.write(Killed {
                victim: ev.target,
                attacker: ev.attacker,
            });
        }
    }
}

fn spawn_shot_fx(
    mut commands: Commands,
    mut events: MessageReader<ShotFx>,
    fx: Res<FxAssets>,
    net: Res<NetMode>,
) {
    let client = net.is_client();
    for ev in events.read() {
        if ev.kind != WeaponKind::Grenade {
            let dir = ev.vel.normalize_or_zero();
            spawn_puff(
                &mut commands,
                &fx,
                &fx.flash,
                ev.origin + dir * 0.15,
                if ev.kind.is_explosive() { 0.35 } else { 0.12 },
                0.05,
                Vec3::ZERO,
                4.0,
            );
        }
        if client {
            spawn_projectile(
                &mut commands,
                &fx,
                ev.kind,
                ev.origin,
                ev.vel,
                None,
                None,
                None,
                ev.tracer,
                true,
            );
        }
    }
}

fn spawn_explosion_fx(
    mut commands: Commands,
    mut events: MessageReader<ExplosionFx>,
    fx: Res<FxAssets>,
) {
    for ev in events.read() {
        let s = ev.size;
        spawn_puff(&mut commands, &fx, &fx.flash, ev.pos, s * 0.25, 0.25, Vec3::ZERO, 6.0);
        spawn_puff(&mut commands, &fx, &fx.fire, ev.pos, s * 0.2, 0.5, Vec3::Y * 2.0, 2.5);
        for _ in 0..(s as usize + 3) {
            let v = Vec3::new(rand_range(-1.0, 1.0), rand_range(0.5, 2.5), rand_range(-1.0, 1.0));
            spawn_puff(
                &mut commands,
                &fx,
                if fastrand::bool() {
                    &fx.smoke_dark
                } else {
                    &fx.dirt
                },
                ev.pos + v * 0.3,
                s * rand_range(0.12, 0.25),
                rand_range(2.0, 4.5),
                v * (s * 0.25),
                0.6,
            );
        }
        commands.spawn((
            PointLight {
                intensity: 400_000.0 * s,
                range: s * 8.0,
                color: Color::srgb(1.0, 0.7, 0.35),
                ..default()
            },
            Transform::from_translation(ev.pos + Vec3::Y),
            Fx {
                life: 0.15,
                max: 0.15,
                grow: 0.0,
                vel: Vec3::ZERO,
                base_scale: 1.0,
            },
            MissionEntity,
        ));
    }
}

fn update_fx(mut commands: Commands, time: Res<Time>, mut q: Query<(Entity, &mut Fx, &mut Transform)>) {
    let dt = time.delta_secs();
    for (e, mut f, mut tf) in &mut q {
        f.life -= dt;
        if f.life <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        tf.translation += f.vel * dt;
        f.vel *= 1.0 - 0.8 * dt;
        let age = 1.0 - f.life / f.max;
        let mut s = f.base_scale * (1.0 + f.grow * age);
        if age > 0.7 {
            s *= 1.0 - (age - 0.7) / 0.3 * 0.8;
        }
        tf.scale = Vec3::splat(s);
    }
}
