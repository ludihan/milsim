//! Squad and soldier AI: orders, formations, perception, hearing and engagement.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::combat::{Gunshot, WeaponKind};
use crate::common::*;
use crate::net::is_authority;
use crate::soldier::{Loadout, Role, Soldier, Stance};
use crate::terrain::{Terrain, WATER_LEVEL};
use crate::vehicle::{EnterVehicle, ExitVehicle, Vehicle};
use crate::world::{Obstacles, line_of_sight};

#[derive(Clone, Debug)]
pub struct TargetInfo {
    pub entity: Entity,
    pub faction: Faction,
    pub pos: Vec3,
    pub aim: Vec3,
    pub vel: Vec3,
    pub is_vehicle: bool,
    /// Multiplier on how far away this can be spotted.
    pub visibility: f32,
    pub dead: bool,
}

/// Snapshot of everything that can be shot at, rebuilt every frame.
#[derive(Resource, Default)]
pub struct Targets {
    pub list: Vec<TargetInfo>,
}

#[derive(Clone, Debug)]
pub enum Order {
    Follow,
    Hold(Vec3),
    Move(Vec3),
    Patrol { points: Vec<Vec3>, idx: usize },
    Attack(Vec3),
    Defend(Vec3, f32),
}

impl Order {
    pub fn label(&self) -> &'static str {
        match self {
            Order::Follow => "FORM ON ME",
            Order::Hold(_) => "HOLD POSITION",
            Order::Move(_) => "MOVE",
            Order::Patrol { .. } => "PATROL",
            Order::Attack(_) => "ATTACK",
            Order::Defend(..) => "DEFEND",
        }
    }
}

pub struct Known {
    pub entity: Entity,
    pub pos: Vec3,
    pub age: f32,
}

#[derive(Component)]
pub struct Squad {
    pub name: String,
    pub faction: Faction,
    pub members: Vec<Entity>,
    pub order: Order,
    pub alert: f32,
    pub known: Vec<Known>,
    pub human_led: bool,
}

impl Squad {
    pub fn new(name: impl Into<String>, faction: Faction, order: Order) -> Self {
        Self {
            name: name.into(),
            faction,
            members: Vec::new(),
            order,
            alert: 0.0,
            known: Vec::new(),
            human_led: false,
        }
    }

    fn remember(&mut self, entity: Entity, pos: Vec3) {
        if let Some(k) = self.known.iter_mut().find(|k| k.entity == entity) {
            k.pos = pos;
            k.age = 0.0;
        } else {
            self.known.push(Known {
                entity,
                pos,
                age: 0.0,
            });
        }
        self.alert = 45.0;
    }
}

#[derive(Component)]
pub struct AiBrain {
    pub skill: f32,
    pub think: f32,
    pub target: Option<Entity>,
    pub visible: bool,
    pub reaction: f32,
    pub aim_err: Vec2,
    pub fire_wait: f32,
    pub burst_time: f32,
    pub slot: Option<Vec3>,
    pub stuck_check: f32,
    pub last_pos: Vec3,
    pub detour: f32,
    pub detour_dir: f32,
    pub stance_timer: f32,
    pub board_timer: f32,
}

impl AiBrain {
    pub fn new(skill: f32) -> Self {
        Self {
            skill,
            think: fastrand::f32() * 0.4,
            target: None,
            visible: false,
            reaction: 0.0,
            aim_err: Vec2::ZERO,
            fire_wait: 0.0,
            burst_time: 0.0,
            slot: None,
            stuck_check: 1.5,
            last_pos: Vec3::ZERO,
            detour: 0.0,
            detour_dir: 1.0,
            stance_timer: 0.0,
            board_timer: 0.0,
        }
    }
}

/// A human (or HQ) gives a squad new orders.
#[derive(Message, Clone)]
pub struct SquadCommand {
    pub squad: Entity,
    pub order: Order,
}

pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Targets>()
            .add_message::<SquadCommand>()
            .add_systems(
                Update,
                (build_targets, squad_update, hearing, soldier_ai)
                    .chain()
                    .in_set(Phase::Ai)
                    .before(crate::vehicle::VehiclePlugin::ai_label())
                    .run_if(is_authority),
            );
    }
}

impl crate::vehicle::VehiclePlugin {
    pub fn ai_label() -> impl SystemSet {
        Phase::Movement
    }
}

/// Point to aim at so a projectile of `kind` from `from` hits a target at `target` moving at `vel`.
pub fn ballistic_aim(from: Vec3, target: Vec3, vel: Vec3, kind: WeaponKind) -> Vec3 {
    let st = kind.stats();
    let dist = from.distance(target);
    let v = st.muzzle_velocity.max(1.0);
    let mut t = dist / v;
    for _ in 0..2 {
        t = dist / (v * (-st.drag * t * 0.5).exp());
    }
    target + vel * t + Vec3::Y * (0.5 * GRAVITY * st.gravity * t * t)
}

fn build_targets(
    mut targets: ResMut<Targets>,
    soldiers: Query<(Entity, &Soldier, &Transform, &Health)>,
    vehicles: Query<(Entity, &Vehicle, &Transform, &Health)>,
) {
    targets.list.clear();
    for (e, s, tf, h) in &soldiers {
        if s.vehicle.is_some() {
            continue;
        }
        let mut vis: f32 = match s.stance {
            Stance::Stand => 1.0,
            Stance::Crouch => 0.7,
            Stance::Prone => 0.4,
        };
        if s.velocity.length() > 1.0 {
            vis *= 1.2;
        }
        if s.since_shot < 1.5 {
            vis *= 2.0;
        }
        targets.list.push(TargetInfo {
            entity: e,
            faction: s.faction,
            pos: tf.translation,
            aim: s.chest(tf.translation),
            vel: s.velocity,
            is_vehicle: false,
            visibility: vis.min(2.0),
            dead: h.dead,
        });
    }
    for (e, v, tf, h) in &vehicles {
        let Some(f) = v.crew_faction else { continue };
        if v.destroyed {
            continue;
        }
        targets.list.push(TargetInfo {
            entity: e,
            faction: f,
            pos: tf.translation,
            aim: v.center(tf),
            vel: v.velocity,
            is_vehicle: true,
            visibility: 2.0,
            dead: h.dead,
        });
    }
}

fn squad_update(
    time: Res<Time>,
    mut commands_in: MessageReader<SquadCommand>,
    mut squads: Query<&mut Squad>,
    soldiers: Query<(&Transform, &Health), With<Soldier>>,
    mut brains: Query<&mut AiBrain>,
) {
    let dt = time.delta_secs();
    for cmd in commands_in.read() {
        if let Ok(mut sq) = squads.get_mut(cmd.squad) {
            sq.order = cmd.order.clone();
            for &m in &sq.members {
                if let Ok(mut b) = brains.get_mut(m) {
                    b.slot = None;
                }
            }
        }
    }
    for mut sq in &mut squads {
        sq.alert = (sq.alert - dt).max(0.0);
        for k in sq.known.iter_mut() {
            k.age += dt;
        }
        sq.known.retain(|k| k.age < 90.0);
        let leader = sq
            .members
            .iter()
            .copied()
            .find(|&m| soldiers.get(m).is_ok_and(|(_, h)| !h.dead));
        let Some(leader) = leader else { continue };
        let Ok((ltf, _)) = soldiers.get(leader) else {
            continue;
        };
        let lpos = ltf.translation;
        let fresh_contact = sq.known.iter().any(|k| k.age < 20.0);
        let next = match &mut sq.order {
            Order::Move(p) if Vec2::new(p.x - lpos.x, p.z - lpos.z).length() < 6.0 => {
                Some(Order::Hold(*p))
            }
            Order::Attack(p) if Vec2::new(p.x - lpos.x, p.z - lpos.z).length() < 15.0 && !fresh_contact => {
                Some(Order::Defend(*p, 45.0))
            }
            Order::Patrol { points, idx } => {
                if let Some(p) = points.get(*idx)
                    && Vec2::new(p.x - lpos.x, p.z - lpos.z).length() < 8.0
                {
                    *idx = (*idx + 1) % points.len();
                }
                None
            }
            _ => None,
        };
        if let Some(n) = next {
            sq.order = n;
        }
    }
}

fn hearing(
    mut shots: MessageReader<Gunshot>,
    mut squads: Query<&mut Squad>,
    soldiers: Query<&Transform, With<Soldier>>,
) {
    for shot in shots.read() {
        for mut sq in &mut squads {
            if sq.faction == shot.faction {
                continue;
            }
            let Some(&first) = sq.members.first() else {
                continue;
            };
            let Ok(tf) = soldiers.get(first) else { continue };
            let d = tf.translation.distance(shot.pos);
            if d < shot.loudness {
                let err = rand_in_circle() * (d * 0.1).min(40.0);
                sq.remember(shot.shooter, shot.pos + Vec3::new(err.x, 0.0, err.y));
            }
        }
    }
}

fn formation_offset(index: usize, spacing: f32) -> Vec3 {
    if index == 0 {
        return Vec3::ZERO;
    }
    let row = index.div_ceil(2) as f32;
    let side = if index % 2 == 1 { 1.0 } else { -1.0 };
    Vec3::new(side * row * spacing, 0.0, row * spacing)
}

struct Info {
    pos: Vec3,
    yaw: f32,
    vehicle: Option<Entity>,
    alive: bool,
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn soldier_ai(
    time: Res<Time>,
    terrain: Res<Terrain>,
    obstacles: Res<Obstacles>,
    targets: Res<Targets>,
    mut squads: Query<&mut Squad>,
    mut soldiers: Query<(
        Entity,
        &mut Soldier,
        &mut AiBrain,
        &Transform,
        &Health,
        &Loadout,
    )>,
    others: Query<(Entity, &Soldier, &Transform, &Health), Without<AiBrain>>,
    vehicles: Query<&Vehicle>,
    mut enters: MessageWriter<EnterVehicle>,
    mut exits: MessageWriter<ExitVehicle>,
) {
    let dt = time.delta_secs();
    let mut info: HashMap<Entity, Info> = HashMap::new();
    for (e, s, tf, h, ..) in soldiers.iter() {
        info.insert(
            e,
            Info {
                pos: tf.translation,
                yaw: s.yaw,
                vehicle: s.vehicle,
                alive: !h.dead,
            },
        );
    }
    for (e, s, tf, h) in &others {
        info.insert(
            e,
            Info {
                pos: tf.translation,
                yaw: s.yaw,
                vehicle: s.vehicle,
                alive: !h.dead,
            },
        );
    }
    let mut spotted: Vec<(Entity, Entity, Vec3)> = Vec::new();

    for (me, mut s, mut brain, tf, health, loadout) in &mut soldiers {
        if health.dead {
            continue;
        }
        let pos = tf.translation;
        let squad_e = s.squad;
        let squad = squad_e.and_then(|q| squads.get(q).ok());
        let alive_members: Vec<Entity> = squad
            .map(|sq| {
                sq.members
                    .iter()
                    .copied()
                    .filter(|m| info.get(m).is_some_and(|i| i.alive))
                    .collect()
            })
            .unwrap_or_default();
        let leader = alive_members.first().copied().unwrap_or(me);
        let is_leader = leader == me;
        let my_index = alive_members.iter().position(|&m| m == me).unwrap_or(0);
        let leader_info = info.get(&leader);
        let leader_vehicle = leader_info.and_then(|i| i.vehicle);
        let human_led = squad.is_some_and(|sq| sq.human_led);
        let order = squad.map(|sq| sq.order.clone()).unwrap_or(Order::Hold(pos));

        // Mounted: AI passengers follow their leader in and out.
        brain.board_timer -= dt;
        if let Some(my_vehicle) = s.vehicle {
            let should_leave = (human_led && !is_leader && leader_vehicle != Some(my_vehicle))
                || vehicles.get(my_vehicle).is_ok_and(|v| v.destroyed);
            if should_leave && brain.board_timer <= 0.0 {
                brain.board_timer = 0.5 + fastrand::f32();
                exits.write(ExitVehicle { soldier: me });
            }
            continue;
        }

        // ---- Perception ----
        brain.think -= dt;
        let eye = s.eye(pos);
        let weapon_kind = loadout.current().kind;
        if brain.think <= 0.0 {
            brain.think = 0.25 + fastrand::f32() * 0.25;
            let mut best: Option<(f32, &TargetInfo)> = None;
            for t in &targets.list {
                if t.faction == s.faction || t.dead {
                    continue;
                }
                let d = t.pos.distance(pos);
                let mut range = 520.0 * t.visibility;
                if squad.is_some_and(|sq| sq.alert > 0.0) {
                    range *= 1.2;
                }
                if d > range {
                    continue;
                }
                let to = (t.pos - pos).normalize_or_zero();
                let facing = forward_of_yaw(s.yaw);
                let in_fov = to.dot(facing) > 0.2 || d < 12.0 + 30.0 * t.visibility.min(1.0);
                if !in_fov {
                    continue;
                }
                let mut score = d / t.visibility;
                if Some(t.entity) == brain.target {
                    score *= 0.5;
                }
                if t.is_vehicle && loadout.launcher_slot().is_some() {
                    score *= 0.4;
                }
                if best.is_some_and(|(b, _)| score >= b) {
                    continue;
                }
                if line_of_sight(&terrain, &obstacles, eye, t.aim + Vec3::Y * 0.2) {
                    best = Some((score, t));
                }
            }
            match best {
                Some((_, t)) => {
                    if brain.target != Some(t.entity) || !brain.visible {
                        let d = t.pos.distance(pos);
                        brain.reaction = 0.3 + fastrand::f32() * 0.6 + d / 500.0 * (1.5 - brain.skill);
                        brain.aim_err = rand_in_circle() * (0.04 + d * 0.0002) * (1.6 - brain.skill);
                    }
                    brain.target = Some(t.entity);
                    brain.visible = true;
                    if let Some(q) = squad_e {
                        spotted.push((q, t.entity, t.pos));
                    }
                }
                None => brain.visible = false,
            }
        }
        let target = brain
            .target
            .and_then(|t| targets.list.iter().find(|x| x.entity == t && !x.dead));
        if target.is_none() {
            brain.target = None;
            brain.visible = false;
        }

        // ---- Engagement ----
        let mut intent = crate::soldier::Intent {
            stance: None,
            ..default()
        };
        let mut engaging = false;
        brain.stance_timer -= dt;
        if let (Some(t), true) = (target, brain.visible) {
            engaging = true;
            let d = t.pos.distance(pos);
            // Pick the right tool.
            let want_launcher = t.is_vehicle && d < 400.0 && loadout.launcher_slot().is_some();
            if want_launcher {
                if let Some(slot) = loadout.launcher_slot()
                    && slot != loadout.active
                {
                    intent.switch_to = Some(slot);
                }
            } else if loadout.current().kind == WeaponKind::Launcher {
                intent.switch_to = Some(0);
            }
            let kind = if want_launcher {
                WeaponKind::Launcher
            } else {
                weapon_kind
            };
            let aim_point = ballistic_aim(eye, t.aim, t.vel, kind);
            let (ty, tp) = angles_of(aim_point - eye);
            let ty = ty + brain.aim_err.x;
            let tp = tp + brain.aim_err.y;
            s.yaw = turn_toward(s.yaw, ty, 5.0 * dt);
            s.pitch += (tp - s.pitch).clamp(-3.0 * dt, 3.0 * dt);
            let decay = (-(0.35 + brain.skill * 0.6) * dt).exp();
            brain.aim_err *= decay;
            let floor = 0.003 + 0.012 * s.suppression;
            if brain.aim_err.length() < floor {
                brain.aim_err += rand_in_circle() * floor;
            }
            brain.reaction -= dt;
            let aligned = angle_diff(s.yaw, ty).abs() < 0.06 && (tp - s.pitch).abs() < 0.06;
            let in_range = d < if kind == WeaponKind::Launcher { 400.0 } else { 650.0 };
            let can_hurt = !t.is_vehicle || kind.stats().ap >= 2 || d < 150.0;
            brain.fire_wait -= dt;
            brain.burst_time -= dt;
            if brain.reaction <= 0.0 && aligned && in_range && can_hurt {
                if brain.burst_time > 0.0 {
                    intent.fire = true;
                } else if brain.fire_wait <= 0.0 {
                    let interval = kind.stats().interval;
                    let rounds = match (s.role, d) {
                        (_, _) if kind == WeaponKind::Launcher => 1.0,
                        (Role::Autorifleman, _) => 4.0 + fastrand::f32() * 5.0,
                        (_, d) if d < 60.0 => 3.0,
                        (_, d) if d < 150.0 => 2.0,
                        _ => 1.0,
                    };
                    brain.burst_time = interval * rounds - 0.01;
                    brain.fire_wait = brain.burst_time
                        + (0.25 + fastrand::f32() * 0.8) * (d / 120.0).clamp(0.6, 2.5)
                        + s.suppression;
                    intent.fire = true;
                }
            }
            intent.ads = true;
            if brain.stance_timer <= 0.0 {
                brain.stance_timer = 2.0 + fastrand::f32() * 3.0;
                let st = if s.suppression > 0.5 || (s.role == Role::Autorifleman && d > 80.0) {
                    Stance::Prone
                } else if d > 220.0 && fastrand::f32() < 0.5 {
                    Stance::Prone
                } else {
                    Stance::Crouch
                };
                intent.stance = Some(st);
            }
        } else {
            // No visible enemy: watch the threat or the last known contact.
            let threat = s.threat_from.take().or_else(|| {
                squad.and_then(|sq| {
                    sq.known
                        .iter()
                        .filter(|k| k.age < 30.0)
                        .min_by(|a, b| {
                            a.pos
                                .distance_squared(pos)
                                .total_cmp(&b.pos.distance_squared(pos))
                        })
                        .map(|k| k.pos)
                })
            });
            if let Some(tp) = threat {
                let (ty, _) = angles_of(tp - pos);
                s.yaw = turn_toward(s.yaw, ty, 3.0 * dt);
                s.pitch *= 1.0 - dt;
                if brain.stance_timer <= 0.0 && s.stance == Stance::Stand {
                    brain.stance_timer = 3.0;
                    intent.stance = Some(Stance::Crouch);
                }
            }
            let w = loadout.current();
            if w.ammo < w.stats().mag_size / 3 && w.reloading <= 0.0 {
                intent.reload = true;
            }
            if loadout.current().kind == WeaponKind::Launcher {
                intent.switch_to = Some(0);
            }
            if loadout.current().ammo == 0 {
                intent.reload = true;
            }
        }
        if s.suppression > 0.7 && brain.stance_timer <= 0.0 {
            intent.stance = Some(Stance::Prone);
            brain.stance_timer = 3.0;
        }

        // ---- Movement goal ----
        let alert = squad.is_some_and(|sq| sq.alert > 0.0);
        let spacing = if alert { 7.0 } else { 5.0 };
        let leader_pos = leader_info.map(|i| i.pos).unwrap_or(pos);
        let leader_yaw = leader_info.map(|i| i.yaw).unwrap_or(0.0);
        let mut goal: Option<Vec3> = None;
        let mut tolerance = 2.0;
        if !is_leader && human_led && let Some(lv) = leader_vehicle {
            if let Ok(v) = vehicles.get(lv)
                && matches!(order, Order::Follow)
                && v.has_seat()
                && !v.destroyed
            {
                let vp = leader_pos;
                goal = Some(vp);
                tolerance = 3.0;
                if pos.distance(vp) < 9.0 && brain.board_timer <= 0.0 {
                    brain.board_timer = 1.0;
                    enters.write(EnterVehicle {
                        soldier: me,
                        vehicle: Some(lv),
                    });
                }
            }
        } else {
            let slot_around = |center: Vec3, radius: f32, brain: &mut AiBrain| -> Vec3 {
                *brain.slot.get_or_insert_with(|| {
                    for _ in 0..12 {
                        let c = rand_in_circle() * radius;
                        let p = center + Vec3::new(c.x, 0.0, c.y);
                        if obstacles.overlap(p.with_y(terrain.height(p.x, p.z)), 0.5, 1.8).is_none()
                            && terrain.height(p.x, p.z) > WATER_LEVEL + 0.5
                        {
                            return p;
                        }
                    }
                    center
                })
            };
            let formation = |anchor: Vec3, yaw: f32| {
                anchor + Quat::from_rotation_y(yaw) * formation_offset(my_index, spacing)
            };
            goal = match &order {
                Order::Defend(c, r) => Some(slot_around(*c, *r, &mut brain)),
                _ if is_leader => match &order {
                    Order::Follow => None,
                    Order::Hold(p) => Some(*p),
                    Order::Move(p) | Order::Attack(p) => Some(*p),
                    Order::Patrol { points, idx } => points.get(*idx).copied(),
                    Order::Defend(..) => unreachable!(),
                },
                Order::Hold(p) if human_led => Some(formation(*p, leader_yaw)),
                Order::Move(p) if human_led => Some(formation(*p, leader_yaw)),
                _ => {
                    tolerance = 2.5;
                    Some(formation(leader_pos, leader_yaw))
                }
            };
        }

        if let Some(g) = goal {
            let to = Vec3::new(g.x - pos.x, 0.0, g.z - pos.z);
            let dist = to.length();
            let far_behind = dist > 30.0;
            let may_move = !engaging || far_behind || (is_leader && matches!(order, Order::Attack(_)) && dist > 15.0);
            if dist > tolerance && may_move {
                let mut dir = to / dist;
                // Detour around whatever we keep bumping into.
                if brain.detour > 0.0 {
                    brain.detour -= dt;
                    dir = Quat::from_rotation_y(brain.detour_dir * 1.1) * dir;
                } else {
                    let probe = pos + dir * 1.6;
                    if obstacles.overlap(probe.with_y(terrain.height(probe.x, probe.z)), 0.4, 1.6).is_some() {
                        brain.detour = 0.8;
                    }
                }
                let ahead = pos + dir * 4.0;
                if terrain.height(ahead.x, ahead.z) < -0.8 {
                    dir = Quat::from_rotation_y(brain.detour_dir * 1.5) * dir;
                }
                intent.move_dir = dir;
                intent.sprint = !engaging && (dist > 18.0 || (alert && dist > 8.0)) && s.stamina > 0.3;
                if engaging {
                    intent.fire = intent.fire && !intent.sprint;
                } else {
                    let (my, _) = angles_of(dir);
                    s.yaw = turn_toward(s.yaw, my, 4.0 * dt);
                    s.pitch *= 1.0 - (2.0 * dt).min(1.0);
                    if !alert && s.stance != Stance::Stand && brain.stance_timer <= 0.0 {
                        intent.stance = Some(Stance::Stand);
                    }
                }
                if s.stance == Stance::Prone && !engaging {
                    intent.stance = Some(if alert { Stance::Crouch } else { Stance::Stand });
                }
                brain.stuck_check -= dt;
                if brain.stuck_check <= 0.0 {
                    brain.stuck_check = 1.5;
                    if pos.distance(brain.last_pos) < 0.6 {
                        brain.detour = 1.4;
                        brain.detour_dir = if fastrand::bool() { 1.0 } else { -1.0 };
                    }
                    brain.last_pos = pos;
                }
            }
        }
        s.intent = intent;
    }

    for (q, target, p) in spotted {
        if let Ok(mut sq) = squads.get_mut(q) {
            sq.remember(target, p);
        }
    }
}
