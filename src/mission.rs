//! Open-world operation: capturable sectors with dynamically simulated garrisons, OPFOR
//! counterattacks, an allied AI squad, and respawning FOB vehicles.

use std::f32::consts::TAU;

use bevy::prelude::*;

use crate::ai::{AiBrain, Order, Squad};
use crate::common::*;
use crate::net::is_authority;
use crate::player::Human;
use crate::soldier::{Role, Soldier, SoldierAssets, random_name, spawn_soldier};
use crate::terrain::{BASE, LOCATIONS, LocKind, Terrain, WATER_LEVEL};
use crate::vehicle::{Vehicle, VehicleAi, VehicleAssets, VehicleKind, spawn_vehicle};
use crate::world::Obstacles;

/// Garrisons spawn when a player gets this close and are cached when everyone leaves.
const ACTIVATE_RANGE: f32 = 1100.0;
const DEACTIVATE_RANGE: f32 = 1500.0;

pub struct Sector {
    pub loc: usize,
    pub owner: Faction,
    /// OPFOR soldiers still holding the sector while it is not simulated.
    pub garrison: u32,
    pub vehicles: Vec<VehicleKind>,
    pub active: bool,
    pub idle: f32,
    pub squads: Vec<Entity>,
    pub spawned_vehicles: Vec<Entity>,
    /// Progress toward changing hands, 0..1.
    pub capture: f32,
    pub counterattack: Option<f32>,
}

#[derive(Resource, Default)]
pub struct Sectors {
    pub list: Vec<Sector>,
    pub won: bool,
}

impl Sectors {
    pub fn new() -> Self {
        let list = LOCATIONS
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != BASE)
            .map(|(i, l)| {
                let (garrison, vehicles) = match (l.kind, l.name) {
                    (_, "Belyy Kamen") => (22, vec![VehicleKind::Ifv, VehicleKind::Jeep]),
                    (LocKind::Town, _) => (16, vec![VehicleKind::Jeep]),
                    (LocKind::Outpost, _) => (12, vec![VehicleKind::Ifv]),
                    _ => (9, vec![]),
                };
                Sector {
                    loc: i,
                    owner: Faction::Opfor,
                    garrison,
                    vehicles,
                    active: false,
                    idle: 0.0,
                    squads: Vec::new(),
                    spawned_vehicles: Vec::new(),
                    capture: 0.0,
                    counterattack: None,
                }
            })
            .collect();
        Self { list, won: false }
    }
}

#[derive(Component)]
pub struct BaseVehicle;

struct BaseSlot {
    kind: VehicleKind,
    offset: Vec2,
    yaw: f32,
    entity: Option<Entity>,
    timer: f32,
}

#[derive(Resource)]
struct Fob {
    slots: Vec<BaseSlot>,
    allied: Option<Entity>,
    allied_timer: f32,
    briefed: bool,
    check: f32,
}

pub struct MissionPlugin;

impl Plugin for MissionPlugin {
    fn build(&self, app: &mut App) {
        let slot = |kind, x: f32, z: f32, yaw: f32| BaseSlot {
            kind,
            offset: Vec2::new(x, z),
            yaw,
            entity: None,
            timer: 0.0,
        };
        app.insert_resource(Sectors::new())
            .insert_resource(Fob {
                slots: vec![
                    slot(VehicleKind::Jeep, 10.0, -20.0, -1.57),
                    slot(VehicleKind::Jeep, 10.0, -12.0, -1.57),
                    slot(VehicleKind::Ifv, 12.0, 0.0, -1.57),
                    slot(VehicleKind::Heli, 25.0, 30.0, -1.57),
                ],
                allied: None,
                allied_timer: 20.0,
                briefed: false,
                check: 0.0,
            })
            .add_systems(
                Update,
                (fob_vehicles, allied_squad, sector_activation, sector_capture, counterattacks)
                    .chain()
                    .in_set(Phase::Ai)
                    .before(crate::ai::build_targets)
                    .run_if(is_authority),
            );
    }
}

fn free_spot(terrain: &Terrain, obstacles: &Obstacles, center: Vec2, radius: f32) -> Vec3 {
    for _ in 0..30 {
        let c = center + rand_in_circle() * radius;
        let h = terrain.height(c.x, c.y);
        let p = Vec3::new(c.x, h, c.y);
        if h > WATER_LEVEL + 0.5 && obstacles.overlap(p, 0.6, 1.8).is_none() {
            return p;
        }
    }
    Vec3::new(center.x, terrain.height(center.x, center.y), center.y)
}

#[allow(clippy::too_many_arguments)]
fn spawn_squad(
    commands: &mut Commands,
    assets: &SoldierAssets,
    terrain: &Terrain,
    obstacles: &Obstacles,
    name: String,
    faction: Faction,
    roles: &[Role],
    at: Vec2,
    spread: f32,
    order: Order,
    skill: f32,
) -> (Entity, Vec<Entity>) {
    let squad = commands.spawn((Squad::new(name, faction, order), MissionEntity)).id();
    let mut members = Vec::new();
    for &role in roles {
        let p = free_spot(terrain, obstacles, at, spread);
        let e = spawn_soldier(
            commands,
            assets,
            random_name(faction, role),
            faction,
            role,
            p,
            fastrand::f32() * TAU,
        );
        commands.entity(e).insert(AiBrain::new(skill));
        members.push(e);
    }
    let ms = members.clone();
    commands.queue(move |world: &mut World| {
        for &m in &ms {
            if let Some(mut s) = world.get_mut::<Soldier>(m) {
                s.squad = Some(squad);
            }
        }
        if let Some(mut q) = world.get_mut::<Squad>(squad) {
            q.members = ms;
        }
    });
    (squad, members)
}

/// Put soldiers into a vehicle (first is the driver) as soon as both exist.
fn crew_vehicle(commands: &mut Commands, vehicle: Entity, crew: Vec<Entity>) {
    commands.queue(move |world: &mut World| {
        let faction = crew
            .first()
            .and_then(|&c| world.get::<Soldier>(c))
            .map(|s| s.faction);
        for (i, &c) in crew.iter().enumerate() {
            if let Some(mut s) = world.get_mut::<Soldier>(c) {
                s.vehicle = Some(vehicle);
            }
            if let Some(mut v) = world.get_mut::<Vehicle>(vehicle) {
                if i == 0 {
                    v.driver = Some(c);
                } else {
                    v.passengers.push(c);
                }
                v.crew_faction = faction;
            }
        }
    });
}

fn opfor_roles(n: usize) -> Vec<Role> {
    let pattern = [
        Role::Leader,
        Role::Autorifleman,
        Role::Rifleman,
        Role::AtSoldier,
        Role::Rifleman,
        Role::Rifleman,
    ];
    (0..n).map(|i| pattern[i % pattern.len()]).collect()
}

#[allow(clippy::too_many_arguments)]
fn fob_vehicles(
    mut commands: Commands,
    time: Res<Time>,
    mut fob: ResMut<Fob>,
    assets: Res<VehicleAssets>,
    terrain: Res<Terrain>,
    vehicles: Query<&Vehicle>,
    mut radio: MessageWriter<Radio>,
) {
    let dt = time.delta_secs();
    if !fob.briefed {
        fob.briefed = true;
        radio.write(Radio::new(
            "HQ: Operation Iron Ridge is a go. Seize every enemy-held sector on the island.",
            RADIO_SIDE,
        ));
        radio.write(Radio::new(
            "HQ: Vehicles are staged at FOB Anvil. Press M for the map.",
            RADIO_SIDE,
        ));
    }
    let base = LOCATIONS[BASE].pos;
    for slot in fob.slots.iter_mut() {
        let alive = slot
            .entity
            .and_then(|e| vehicles.get(e).ok())
            .is_some_and(|v| !v.destroyed);
        if alive {
            continue;
        }
        if slot.entity.is_some() && slot.timer <= 0.0 {
            slot.timer = 90.0;
        }
        slot.timer -= dt;
        if slot.timer > 0.0 && slot.entity.is_some() {
            continue;
        }
        if let Some(old) = slot.entity.take() {
            commands.entity(old).despawn();
            radio.write(Radio::new("HQ: Replacement vehicle available at the FOB.", RADIO_SIDE));
        }
        let p = base + slot.offset;
        let pos = Vec3::new(p.x, terrain.height(p.x, p.y), p.y);
        let e = spawn_vehicle(&mut commands, &assets, slot.kind, Faction::Blufor, pos, slot.yaw);
        commands.entity(e).insert(BaseVehicle);
        slot.entity = Some(e);
        slot.timer = 0.0;
    }
}

/// An allied AI squad keeps pushing on the nearest enemy sector.
#[allow(clippy::too_many_arguments)]
fn allied_squad(
    mut commands: Commands,
    time: Res<Time>,
    mut fob: ResMut<Fob>,
    sectors: Res<Sectors>,
    assets: Res<SoldierAssets>,
    terrain: Res<Terrain>,
    obstacles: Res<Obstacles>,
    mut squads: Query<&mut Squad>,
    soldiers: Query<(&Health, &Transform), With<Soldier>>,
    mut radio: MessageWriter<Radio>,
) {
    let alive_pos = fob.allied.and_then(|s| squads.get(s).ok()).and_then(|sq| {
        sq.members
            .iter()
            .filter_map(|&m| soldiers.get(m).ok())
            .find(|(h, _)| !h.dead)
            .map(|(_, tf)| tf.translation)
    });
    let from = alive_pos.map_or(LOCATIONS[BASE].pos, |p| Vec2::new(p.x, p.z));
    let target = sectors
        .list
        .iter()
        .filter(|s| s.owner == Faction::Opfor)
        .min_by(|a, b| {
            LOCATIONS[a.loc]
                .pos
                .distance(from)
                .total_cmp(&LOCATIONS[b.loc].pos.distance(from))
        })
        .map(|s| s.loc);
    match (alive_pos, fob.allied) {
        (Some(_), Some(sq_e)) => {
            if let Ok(mut sq) = squads.get_mut(sq_e)
                && let Some(t) = target
            {
                let tp = LOCATIONS[t].pos;
                let goal = Vec3::new(tp.x, terrain.height(tp.x, tp.y), tp.y);
                let retarget = match &sq.order {
                    Order::Attack(p) | Order::Defend(p, _) => p.distance(goal) > 50.0,
                    _ => true,
                };
                if retarget {
                    sq.order = Order::Attack(goal);
                    radio.write(Radio::new(
                        format!("{}: Moving to attack {}.", sq.name, LOCATIONS[t].name),
                        RADIO_SIDE,
                    ));
                }
            }
        }
        _ => {
            fob.allied_timer -= time.delta_secs();
            if fob.allied_timer > 0.0 || target.is_none() {
                return;
            }
            fob.allied_timer = 180.0;
            if let Some(old) = fob.allied.take() {
                commands.entity(old).despawn();
            }
            let (sq, _) = spawn_squad(
                &mut commands,
                &assets,
                &terrain,
                &obstacles,
                "Bravo 2-1".into(),
                Faction::Blufor,
                &[
                    Role::Leader,
                    Role::Autorifleman,
                    Role::Rifleman,
                    Role::AtSoldier,
                    Role::Rifleman,
                    Role::Rifleman,
                ],
                LOCATIONS[BASE].pos + Vec2::new(-30.0, 20.0),
                10.0,
                Order::Hold(Vec3::ZERO),
                0.7,
            );
            fob.allied = Some(sq);
            radio.write(Radio::new("Bravo 2-1 deployed from FOB Anvil.", RADIO_SIDE));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn sector_activation(
    mut commands: Commands,
    time: Res<Time>,
    mut sectors: ResMut<Sectors>,
    mut fob: ResMut<Fob>,
    assets: Res<SoldierAssets>,
    vassets: Res<VehicleAssets>,
    terrain: Res<Terrain>,
    obstacles: Res<Obstacles>,
    humans: Query<(&Transform, &Health), With<Human>>,
    squads: Query<&Squad>,
    soldiers: Query<(&Health, Option<&Corpse>), With<Soldier>>,
    vehicles: Query<&Vehicle>,
) {
    let dt = time.delta_secs();
    fob.check -= dt;
    if fob.check > 0.0 {
        return;
    }
    fob.check = 1.0;
    let players: Vec<Vec2> = humans
        .iter()
        .filter(|(_, h)| !h.dead)
        .map(|(tf, _)| Vec2::new(tf.translation.x, tf.translation.z))
        .collect();
    for sector in sectors.list.iter_mut() {
        let loc = &LOCATIONS[sector.loc];
        let nearest = players
            .iter()
            .map(|p| p.distance(loc.pos))
            .fold(f32::MAX, f32::min);
        if !sector.active {
            if nearest < ACTIVATE_RANGE && sector.owner == Faction::Opfor && sector.garrison > 0 {
                sector.active = true;
                sector.idle = 0.0;
                spawn_garrison(&mut commands, &assets, &vassets, &terrain, &obstacles, sector);
            }
            continue;
        }
        if nearest > DEACTIVATE_RANGE {
            sector.idle += 1.0;
        } else {
            sector.idle = 0.0;
        }
        if sector.idle < 30.0 {
            continue;
        }
        // Cache the survivors and despawn the garrison.
        let mut alive = 0;
        for &sq in &sector.squads {
            if let Ok(squad) = squads.get(sq) {
                for &m in &squad.members {
                    if soldiers.get(m).is_ok_and(|(h, _)| !h.dead) {
                        alive += 1;
                        commands.entity(m).despawn();
                    }
                }
            }
            commands.entity(sq).despawn();
        }
        let mut kept = Vec::new();
        for &v in &sector.spawned_vehicles {
            if let Ok(veh) = vehicles.get(v)
                && !veh.destroyed
                && veh.crew_faction != Some(Faction::Blufor)
            {
                if let Some(k) = Some(veh.kind) {
                    kept.push(k);
                }
                for occ in veh.occupants() {
                    commands.entity(occ).despawn();
                }
                commands.entity(v).despawn();
            }
        }
        sector.vehicles = kept;
        sector.squads.clear();
        sector.spawned_vehicles.clear();
        sector.garrison = alive;
        sector.active = false;
    }
}

use crate::soldier::Corpse;

fn spawn_garrison(
    commands: &mut Commands,
    assets: &SoldierAssets,
    vassets: &VehicleAssets,
    terrain: &Terrain,
    obstacles: &Obstacles,
    sector: &mut Sector,
) {
    let loc = &LOCATIONS[sector.loc];
    let mut remaining = sector.garrison as usize;
    let mut k = 0;
    while remaining > 0 {
        let n = remaining.min(6);
        remaining -= n;
        let name = format!("{} {}", loc.name, ["Guard", "Watch", "Patrol", "Reserve"][k % 4]);
        let center3 = Vec3::new(loc.pos.x, terrain.height(loc.pos.x, loc.pos.y), loc.pos.y);
        let order = if k == 2 {
            let r = loc.radius * 1.4;
            let points = (0..6)
                .map(|i| {
                    let a = i as f32 / 6.0 * TAU;
                    let p = loc.pos + Vec2::new(a.cos(), a.sin()) * r;
                    Vec3::new(p.x, terrain.height(p.x, p.y), p.y)
                })
                .collect();
            Order::Patrol { points, idx: 0 }
        } else {
            Order::Defend(center3, loc.radius * 0.75)
        };
        let (sq, _) = spawn_squad(
            commands,
            assets,
            terrain,
            obstacles,
            name,
            Faction::Opfor,
            &opfor_roles(n),
            loc.pos,
            loc.radius * 0.7,
            order,
            0.55,
        );
        sector.squads.push(sq);
        k += 1;
    }
    for (i, &kind) in sector.vehicles.iter().enumerate() {
        let p = free_spot(terrain, obstacles, loc.pos + Vec2::new(12.0 + i as f32 * 9.0, 0.0), 8.0);
        let v = spawn_vehicle(commands, vassets, kind, Faction::Opfor, p, fastrand::f32() * TAU);
        let r = loc.radius * 1.1;
        let route = if kind == VehicleKind::Ifv {
            Vec::new()
        } else {
            (0..8)
                .map(|j| {
                    let a = j as f32 / 8.0 * TAU;
                    let q = loc.pos + Vec2::new(a.cos(), a.sin()) * r;
                    Vec3::new(q.x, terrain.height(q.x, q.y), q.y)
                })
                .collect()
        };
        commands.entity(v).insert(VehicleAi {
            route,
            looping: true,
            ..default()
        });
        let (sq, crew) = spawn_squad(
            commands,
            assets,
            terrain,
            obstacles,
            format!("{} Crew", loc.name),
            Faction::Opfor,
            &[Role::Crew],
            Vec2::new(p.x, p.z),
            2.0,
            Order::Hold(p),
            0.55,
        );
        crew_vehicle(commands, v, crew);
        sector.squads.push(sq);
        sector.spawned_vehicles.push(v);
    }
}

#[allow(clippy::too_many_arguments)]
fn sector_capture(
    time: Res<Time>,
    mut sectors: ResMut<Sectors>,
    soldiers: Query<(&Soldier, &Health, &Transform)>,
    mut radio: MessageWriter<Radio>,
) {
    let dt = time.delta_secs();
    let mut counts = vec![(0u32, 0u32); sectors.list.len()];
    for (s, h, tf) in &soldiers {
        if h.dead {
            continue;
        }
        let p = Vec2::new(tf.translation.x, tf.translation.z);
        for (i, sec) in sectors.list.iter().enumerate() {
            let loc = &LOCATIONS[sec.loc];
            if p.distance(loc.pos) < loc.radius * 1.2 {
                match s.faction {
                    Faction::Blufor => counts[i].0 += 1,
                    Faction::Opfor => counts[i].1 += 1,
                }
            }
        }
    }
    for (i, sec) in sectors.list.iter_mut().enumerate() {
        let (blu, op) = counts[i];
        let name = LOCATIONS[sec.loc].name;
        let cached_opfor = !sec.active && sec.garrison > 0;
        match sec.owner {
            Faction::Opfor => {
                if blu > 0 && op == 0 && !cached_opfor {
                    sec.capture += dt / 20.0;
                } else {
                    sec.capture = (sec.capture - dt / 40.0).max(0.0);
                }
                if sec.capture >= 1.0 {
                    sec.capture = 0.0;
                    sec.owner = Faction::Blufor;
                    sec.garrison = 0;
                    sec.counterattack = Some(rand_range(60.0, 150.0));
                    radio.write(Radio::new(format!("HQ: {name} is secure. Good work."), RADIO_SYSTEM));
                }
            }
            Faction::Blufor => {
                if op > 0 && blu == 0 {
                    sec.capture += dt / 30.0;
                } else {
                    sec.capture = (sec.capture - dt / 30.0).max(0.0);
                }
                if sec.capture >= 1.0 {
                    sec.capture = 0.0;
                    sec.owner = Faction::Opfor;
                    sec.garrison = op;
                    radio.write(Radio::new(format!("HQ: We've lost {name}!"), RADIO_KILL));
                }
            }
        }
    }
    let all = sectors.list.iter().all(|s| s.owner == Faction::Blufor);
    if all && !sectors.won {
        radio.write(Radio::new(
            "HQ: All sectors secured. The island is ours. Mission accomplished!",
            RADIO_SYSTEM,
        ));
    }
    sectors.won = all;
}

/// After a sector falls, OPFOR sends a motorized QRF from its nearest remaining sector.
#[allow(clippy::too_many_arguments)]
fn counterattacks(
    mut commands: Commands,
    time: Res<Time>,
    mut sectors: ResMut<Sectors>,
    assets: Res<SoldierAssets>,
    vassets: Res<VehicleAssets>,
    terrain: Res<Terrain>,
    obstacles: Res<Obstacles>,
    humans: Query<&Transform, With<Human>>,
    mut radio: MessageWriter<Radio>,
) {
    let dt = time.delta_secs();
    let opfor_locs: Vec<usize> = sectors
        .list
        .iter()
        .filter(|s| s.owner == Faction::Opfor)
        .map(|s| s.loc)
        .collect();
    for sec in sectors.list.iter_mut() {
        let Some(t) = sec.counterattack.as_mut() else { continue };
        *t -= dt;
        if *t > 0.0 {
            continue;
        }
        sec.counterattack = None;
        if sec.owner != Faction::Blufor {
            continue;
        }
        let target = LOCATIONS[sec.loc].pos;
        let watched = humans
            .iter()
            .any(|tf| Vec2::new(tf.translation.x, tf.translation.z).distance(target) < DEACTIVATE_RANGE);
        let Some(&from) = opfor_locs
            .iter()
            .min_by(|&&a, &&b| LOCATIONS[a].pos.distance(target).total_cmp(&LOCATIONS[b].pos.distance(target)))
        else {
            continue;
        };
        if !watched {
            continue;
        }
        // Stage the QRF part-way so it arrives in a couple of minutes.
        let src = LOCATIONS[from].pos;
        let start = target + (src - target).normalize_or(Vec2::X) * (src.distance(target)).min(900.0);
        let start3 = free_spot(&terrain, &obstacles, start, 30.0);
        let goal = Vec3::new(target.x, terrain.height(target.x, target.y), target.y);
        let kind = if fastrand::bool() { VehicleKind::Ifv } else { VehicleKind::Jeep };
        let v = spawn_vehicle(&mut commands, &vassets, kind, Faction::Opfor, start3, 0.0);
        let mid = start3.lerp(goal, 0.5);
        let stop = goal + (start3 - goal).normalize_or(Vec3::X) * 120.0;
        commands.entity(v).insert(VehicleAi {
            route: vec![mid, stop.with_y(terrain.height(stop.x, stop.z))],
            dismount_at_end: true,
            ..default()
        });
        let seats = if kind == VehicleKind::Ifv { 6 } else { 4 };
        let (_, mut crew) = spawn_squad(
            &mut commands,
            &assets,
            &terrain,
            &obstacles,
            format!("QRF {} Crew", LOCATIONS[from].name),
            Faction::Opfor,
            &[Role::Crew],
            Vec2::new(start3.x, start3.z),
            3.0,
            Order::Hold(goal),
            0.6,
        );
        let (_, infantry) = spawn_squad(
            &mut commands,
            &assets,
            &terrain,
            &obstacles,
            format!("QRF {}", LOCATIONS[from].name),
            Faction::Opfor,
            &opfor_roles(seats - 1),
            Vec2::new(start3.x, start3.z),
            3.0,
            Order::Attack(goal),
            0.6,
        );
        crew.extend(infantry);
        crew_vehicle(&mut commands, v, crew);
        radio.write(Radio::new(
            format!("HQ: Enemy QRF inbound to {} from the direction of {}!", LOCATIONS[sec.loc].name, LOCATIONS[from].name),
            RADIO_KILL,
        ));
    }
}
