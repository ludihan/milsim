//! Host-authoritative co-op over UDP.
//!
//! Clients send their input every frame; the host simulates everything and sends 20 Hz snapshots
//! (entity states, the player's private HUD state, and an event stream for shots, sounds and
//! effects). Clients interpolate between snapshots and simulate nothing.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{SocketAddr, UdpSocket};

use bevy::prelude::*;

use crate::audio::{PlaySound, SoundKind};
use crate::combat::{ExplosionFx, ShotFx, WeaponKind};
use crate::common::*;
use crate::mission::Sectors;
use crate::player::{
    EDGE_COUNT, Human, HumanSlot, Humans, LocalInput, LocalState, PlayerInput, PrivateState,
    private_state,
};
use crate::ai::{AiBrain, Squad};
use crate::soldier::{Loadout, Role, Soldier, SoldierAssets, Stance, spawn_soldier};
use crate::terrain::Terrain;
use crate::vehicle::{Vehicle, VehicleAssets, VehicleKind, spawn_vehicle};
use crate::world::TreeFell;

pub const DEFAULT_PORT: u16 = 27015;
const SNAPSHOT_INTERVAL: f32 = 0.05;
const TIMEOUT: f64 = 10.0;
const MAX_PACKET: usize = 65_000;

const C_HELLO: u8 = 0;
const C_INPUT: u8 = 1;
const C_BYE: u8 = 2;
const S_WELCOME: u8 = 10;
const S_SNAPSHOT: u8 = 11;

#[derive(Resource)]
pub enum NetMode {
    Single,
    Host(HostNet),
    Client(ClientNet),
}

impl NetMode {
    pub fn is_client(&self) -> bool {
        matches!(self, NetMode::Client(_))
    }
}

pub fn is_authority(net: Res<NetMode>) -> bool {
    !net.is_client()
}

pub struct HostNet {
    socket: UdpSocket,
    clients: Vec<ClientConn>,
    next_id: u32,
    send_timer: f32,
    tick: u32,
    events: Vec<(u32, NetEvent, u8)>,
    next_seq: u32,
}

struct ClientConn {
    addr: SocketAddr,
    id: u32,
    last_heard: f64,
}

pub struct ClientNet {
    socket: UdpSocket,
    server: SocketAddr,
    name: String,
    pub id: Option<u32>,
    hello_timer: f32,
    send_timer: f32,
    last_tick: u32,
    pub last_snapshot: f64,
    seen: HashSet<u32>,
    seen_order: VecDeque<u32>,
}

impl NetMode {
    pub fn host(port: u16) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", port))?;
        socket.set_nonblocking(true)?;
        Ok(NetMode::Host(HostNet {
            socket,
            clients: Vec::new(),
            next_id: 1,
            send_timer: 0.0,
            tick: 0,
            events: Vec::new(),
            next_seq: 1,
        }))
    }

    pub fn join(server: SocketAddr, name: String) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", 0))?;
        socket.set_nonblocking(true)?;
        Ok(NetMode::Client(ClientNet {
            socket,
            server,
            name,
            id: None,
            hello_timer: 0.0,
            send_timer: 0.0,
            last_tick: 0,
            last_snapshot: 0.0,
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
        }))
    }
}

/// Replication id shared by host and clients.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NetId(pub u32);

#[derive(Resource, Default)]
pub struct NetIndex {
    pub map: HashMap<u32, Entity>,
    next: u32,
}

/// Client-side smoothing between snapshots.
#[derive(Component)]
struct Interp {
    from: Vec3,
    to: Vec3,
    from_rot: Quat,
    to_rot: Quat,
    t: f32,
}

#[derive(Clone)]
enum NetEvent {
    Shot(ShotFx),
    Explosion(ExplosionFx),
    Sound(PlaySound),
    Radio(Radio),
    Tree(TreeFell),
}

// ---------------------------------------------------------------------------
// Binary encoding
// ---------------------------------------------------------------------------

#[derive(Default)]
struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn v2(&mut self, v: Vec2) {
        self.f32(v.x);
        self.f32(v.y);
    }
    fn v3(&mut self, v: Vec3) {
        self.f32(v.x);
        self.f32(v.y);
        self.f32(v.z);
    }
    fn quat(&mut self, q: Quat) {
        for c in q.to_array() {
            self.f32(c);
        }
    }
    fn str(&mut self, s: &str) {
        let b = &s.as_bytes()[..s.len().min(255)];
        self.u8(b.len() as u8);
        self.0.extend_from_slice(b);
    }
    fn bool(&mut self, b: bool) {
        self.u8(b as u8);
    }
}

struct R<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.p..self.p + n)?;
        self.p += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        let v = f32::from_le_bytes(self.take(4)?.try_into().ok()?);
        v.is_finite().then_some(v)
    }
    fn v2(&mut self) -> Option<Vec2> {
        Some(Vec2::new(self.f32()?, self.f32()?))
    }
    fn v3(&mut self) -> Option<Vec3> {
        Some(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }
    fn quat(&mut self) -> Option<Quat> {
        Some(Quat::from_array([self.f32()?, self.f32()?, self.f32()?, self.f32()?]).normalize())
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        Some(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn bool(&mut self) -> Option<bool> {
        Some(self.u8()? != 0)
    }
}

fn write_input(w: &mut W, i: &PlayerInput) {
    w.v2(i.move_axis);
    w.f32(i.yaw);
    w.f32(i.pitch);
    let flags = [
        i.sprint,
        i.fire,
        i.ads,
        i.up,
        i.down,
        i.roll_left,
        i.roll_right,
        i.brake,
    ]
    .iter()
    .enumerate()
    .fold(0u16, |acc, (k, &b)| acc | ((b as u16) << k));
    w.u16(flags);
    w.v3(i.aim_point);
    w.u16(i.zeroing);
    w.u8(i.role);
    for e in i.edges {
        w.u8(e);
    }
}

fn read_input(r: &mut R) -> Option<PlayerInput> {
    let move_axis = r.v2()?.clamp(Vec2::splat(-1.0), Vec2::splat(1.0));
    let yaw = r.f32()?;
    let pitch = r.f32()?.clamp(-1.6, 1.6);
    let flags = r.u16()?;
    let bit = |k: u16| flags & (1 << k) != 0;
    let aim_point = r.v3()?;
    let zeroing = r.u16()?.clamp(100, 800);
    let role = r.u8()?;
    let mut edges = [0u8; EDGE_COUNT];
    for e in edges.iter_mut() {
        *e = r.u8()?;
    }
    Some(PlayerInput {
        move_axis,
        yaw,
        pitch,
        sprint: bit(0),
        fire: bit(1),
        ads: bit(2),
        up: bit(3),
        down: bit(4),
        roll_left: bit(5),
        roll_right: bit(6),
        brake: bit(7),
        aim_point,
        zeroing,
        role,
        edges,
    })
}

fn write_private(w: &mut W, s: &PrivateState) {
    w.u32(s.soldier_id);
    w.bool(s.alive);
    w.f32(s.hp);
    w.f32(s.stamina);
    w.u8(s.stance);
    w.u8(s.weapons.len() as u8);
    for &k in &s.weapons {
        w.u8(k);
    }
    w.u8(s.active);
    w.u32(s.ammo);
    w.u32(s.mags);
    w.bool(s.reloading);
    w.bool(s.auto_mode);
    w.u32(s.grenades);
    w.u32(s.fak);
    w.bool(s.healing);
    w.u32(s.shots);
    w.u32(s.hurt);
    w.u32(s.hit);
    w.f32(s.respawn_timer);
    w.u32(s.kills);
    w.u32(s.deaths);
    w.bool(s.in_vehicle);
    w.bool(s.driver);
    w.u8(s.vehicle_kind);
    w.u8(s.vehicle_paint);
    w.f32(s.vehicle_hp);
    w.f32(s.vehicle_speed);
    w.f32(s.vehicle_alt);
    w.u8(s.vehicle_weapon);
    w.u32(s.vehicle_ammo);
    w.u32(s.vehicle_mags);
    w.bool(s.vehicle_reloading);
    w.str(&s.squad_name);
    w.str(&s.order);
    w.u8(s.squad.len() as u8);
    for (name, role, alive, id) in &s.squad {
        w.str(name);
        w.u8(*role);
        w.bool(*alive);
        w.u32(*id);
    }
}

fn read_private(r: &mut R) -> Option<PrivateState> {
    let mut s = PrivateState {
        soldier_id: r.u32()?,
        alive: r.bool()?,
        hp: r.f32()?,
        stamina: r.f32()?,
        stance: r.u8()?,
        ..default()
    };
    let n = r.u8()?;
    for _ in 0..n {
        s.weapons.push(r.u8()?);
    }
    s.active = r.u8()?;
    s.ammo = r.u32()?;
    s.mags = r.u32()?;
    s.reloading = r.bool()?;
    s.auto_mode = r.bool()?;
    s.grenades = r.u32()?;
    s.fak = r.u32()?;
    s.healing = r.bool()?;
    s.shots = r.u32()?;
    s.hurt = r.u32()?;
    s.hit = r.u32()?;
    s.respawn_timer = r.f32()?;
    s.kills = r.u32()?;
    s.deaths = r.u32()?;
    s.in_vehicle = r.bool()?;
    s.driver = r.bool()?;
    s.vehicle_kind = r.u8()?;
    s.vehicle_paint = r.u8()?;
    s.vehicle_hp = r.f32()?;
    s.vehicle_speed = r.f32()?;
    s.vehicle_alt = r.f32()?;
    s.vehicle_weapon = r.u8()?;
    s.vehicle_ammo = r.u32()?;
    s.vehicle_mags = r.u32()?;
    s.vehicle_reloading = r.bool()?;
    s.squad_name = r.str()?;
    s.order = r.str()?;
    let n = r.u8()?;
    for _ in 0..n {
        s.squad.push((r.str()?, r.u8()?, r.bool()?, r.u32()?));
    }
    Some(s)
}

fn color_bytes(c: Color) -> [u8; 3] {
    let s = c.to_srgba();
    [
        (s.red * 255.0) as u8,
        (s.green * 255.0) as u8,
        (s.blue * 255.0) as u8,
    ]
}

fn write_event(w: &mut W, seq: u32, e: &NetEvent) {
    w.u32(seq);
    match e {
        NetEvent::Shot(s) => {
            w.u8(0);
            w.v3(s.origin);
            w.v3(s.vel);
            w.u8(s.kind as u8);
            w.bool(s.tracer);
        }
        NetEvent::Explosion(x) => {
            w.u8(1);
            w.v3(x.pos);
            w.f32(x.size);
        }
        NetEvent::Sound(s) => {
            w.u8(2);
            w.u8(s.kind as u8);
            w.v3(s.pos);
            w.f32(s.volume);
        }
        NetEvent::Radio(rm) => {
            w.u8(3);
            for b in color_bytes(rm.color) {
                w.u8(b);
            }
            w.str(&rm.text);
        }
        NetEvent::Tree(t) => {
            w.u8(4);
            w.u32(t.obstacle);
            w.v2(t.dir);
        }
    }
}

fn read_event(r: &mut R) -> Option<(u32, NetEvent)> {
    let seq = r.u32()?;
    let e = match r.u8()? {
        0 => NetEvent::Shot(ShotFx {
            origin: r.v3()?,
            vel: r.v3()?,
            kind: WeaponKind::from_u8(r.u8()?),
            tracer: r.bool()?,
        }),
        1 => NetEvent::Explosion(ExplosionFx {
            pos: r.v3()?,
            size: r.f32()?,
        }),
        2 => NetEvent::Sound(PlaySound {
            kind: SoundKind::from_u8(r.u8()?),
            pos: r.v3()?,
            volume: r.f32()?,
            local_only: false,
        }),
        3 => {
            let (cr, cg, cb) = (r.u8()?, r.u8()?, r.u8()?);
            NetEvent::Radio(Radio::new(r.str()?, Color::srgb_u8(cr, cg, cb)))
        }
        4 => NetEvent::Tree(TreeFell {
            obstacle: r.u32()?,
            dir: r.v2()?,
        }),
        _ => return None,
    };
    Some((seq, e))
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetIndex>()
            .add_systems(Update, client_receive.in_set(Phase::Input).run_if(is_client))
            .add_systems(Update, host_receive.in_set(Phase::Input).before(crate::player::apply_human_inputs).run_if(is_host))
            .add_systems(Update, assign_net_ids.in_set(Phase::Damage).run_if(is_authority))
            .add_systems(
                Update,
                (collect_events, host_send)
                    .chain()
                    .in_set(Phase::Hud)
                    .run_if(is_host),
            )
            .add_systems(
                Update,
                (client_send, client_interpolate)
                    .in_set(Phase::Camera)
                    .run_if(is_client),
            )
            .add_systems(Last, say_goodbye.run_if(is_client));
    }
}

fn is_client(net: Res<NetMode>) -> bool {
    net.is_client()
}

fn is_host(net: Res<NetMode>) -> bool {
    matches!(*net, NetMode::Host(_))
}

fn assign_net_ids(
    mut commands: Commands,
    mut index: ResMut<NetIndex>,
    new: Query<Entity, (Or<(With<Soldier>, With<Vehicle>)>, Without<NetId>)>,
    mut removed: RemovedComponents<NetId>,
    ids: Query<&NetId>,
) {
    for e in &new {
        index.next += 1;
        let id = index.next;
        index.map.insert(id, e);
        commands.entity(e).insert(NetId(id));
    }
    let gone: Vec<Entity> = removed.read().collect();
    if !gone.is_empty() {
        index.map.retain(|_, e| !gone.contains(e) || ids.contains(*e));
    }
}

#[allow(clippy::too_many_arguments)]
fn host_receive(
    mut net: ResMut<NetMode>,
    time: Res<Time>,
    mut humans: ResMut<Humans>,
    mut radio: MessageWriter<Radio>,
    mut commands: Commands,
    soldiers: Query<&Soldier>,
) {
    let NetMode::Host(host) = &mut *net else { return };
    let now = time.elapsed_secs_f64();
    let mut buf = [0u8; 2048];
    loop {
        let (n, addr) = match host.socket.recv_from(&mut buf) {
            Ok(x) => x,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        };
        let mut r = R::new(&buf[..n]);
        let Some(kind) = r.u8() else { continue };
        let existing = host.clients.iter_mut().find(|c| c.addr == addr);
        match kind {
            C_HELLO => {
                let name: String = r
                    .str()
                    .unwrap_or_default()
                    .chars()
                    .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                    .take(16)
                    .collect();
                let id = match existing {
                    Some(c) => {
                        c.last_heard = now;
                        c.id
                    }
                    None => {
                        let id = host.next_id;
                        host.next_id += 1;
                        host.clients.push(ClientConn {
                            addr,
                            id,
                            last_heard: now,
                        });
                        let name = if name.is_empty() {
                            format!("Player{id}")
                        } else {
                            name
                        };
                        radio.write(Radio::new(format!("{name} joined the operation"), RADIO_SYSTEM));
                        humans.add(id, name);
                        id
                    }
                };
                let mut w = W::default();
                w.u8(S_WELCOME);
                w.u32(id);
                let _ = host.socket.send_to(&w.0, addr);
            }
            C_INPUT => {
                let Some(c) = existing else { continue };
                c.last_heard = now;
                let id = c.id;
                if let Some(input) = read_input(&mut r)
                    && let Some(slot) = humans.get_mut(id)
                {
                    slot.input = input;
                }
            }
            C_BYE => {
                if let Some(c) = existing {
                    c.last_heard = -1000.0;
                }
            }
            _ => {}
        }
    }
    // Drop silent clients; their soldier carries on under AI control.
    let dropped: Vec<u32> = host
        .clients
        .iter()
        .filter(|c| now - c.last_heard > TIMEOUT)
        .map(|c| c.id)
        .collect();
    host.clients.retain(|c| now - c.last_heard <= TIMEOUT);
    for id in dropped {
        if let Some(slot) = humans.get_mut(id) {
            slot.connected = false;
            radio.write(Radio::new(format!("{} left the operation", slot.name), RADIO_SYSTEM));
            if let Some(e) = slot.soldier.take()
                && soldiers.contains(e)
            {
                commands.entity(e).remove::<Human>().insert(AiBrain::new(0.7));
            }
        }
        humans.slots.retain(|s| s.id != id);
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_events(
    mut net: ResMut<NetMode>,
    mut shots: MessageReader<ShotFx>,
    mut explosions: MessageReader<ExplosionFx>,
    mut sounds: MessageReader<PlaySound>,
    mut radio: MessageReader<Radio>,
    mut trees: MessageReader<TreeFell>,
) {
    let NetMode::Host(host) = &mut *net else { return };
    let mut push = |e: NetEvent, sends: u8| {
        host.events.push((host.next_seq, e, sends));
        host.next_seq += 1;
    };
    for s in shots.read() {
        push(NetEvent::Shot(*s), 1);
    }
    for s in explosions.read() {
        push(NetEvent::Explosion(*s), 2);
    }
    for s in sounds.read() {
        if !s.local_only {
            push(NetEvent::Sound(*s), 1);
        }
    }
    for s in radio.read() {
        push(NetEvent::Radio(s.clone()), 3);
    }
    for s in trees.read() {
        push(NetEvent::Tree(*s), 3);
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn host_send(
    mut net: ResMut<NetMode>,
    time: Res<Time>,
    humans: Res<Humans>,
    soldiers_full: Query<(&Soldier, &Loadout, &Health, &Transform)>,
    soldiers: Query<(&NetId, &Soldier, &Health, &Transform)>,
    vehicles: Query<(&NetId, &Vehicle, &Health, &Transform)>,
    vehicles_full: Query<(&Vehicle, &Health, &Transform), Without<Soldier>>,
    ids: Query<&NetId>,
    squads: Query<&Squad>,
    terrain: Res<Terrain>,
    sectors: Res<Sectors>,
) {
    let NetMode::Host(host) = &mut *net else { return };
    host.send_timer -= time.delta_secs();
    if host.send_timer > 0.0 {
        return;
    }
    host.send_timer = SNAPSHOT_INTERVAL;
    host.tick += 1;
    if host.clients.is_empty() {
        host.events.clear();
        return;
    }
    let netid = |e: Option<Entity>| e.and_then(|e| ids.get(e).ok()).map_or(0, |n| n.0);

    let mut shared = W::default();
    shared.u16(soldiers.iter().count() as u16);
    for (id, s, h, tf) in &soldiers {
        shared.u32(id.0);
        shared.u8(s.faction as u8);
        shared.u8(s.role as u8);
        shared.u8(s.stance as u8);
        shared.u8((h.dead as u8) | ((s.sprinting as u8) << 1));
        shared.u8(s.held as u8);
        shared.v3(tf.translation);
        shared.f32(s.yaw);
        shared.f32(s.pitch);
        shared.u32(netid(s.vehicle));
        shared.str(&s.name);
    }
    shared.u16(vehicles.iter().count() as u16);
    for (id, v, h, tf) in &vehicles {
        shared.u32(id.0);
        shared.u8(v.kind as u8);
        shared.u8(v.paint as u8);
        let crew = match v.crew_faction {
            None => 0,
            Some(Faction::Blufor) => 1,
            Some(Faction::Opfor) => 2,
        };
        shared.u8((v.destroyed as u8) | (crew << 1));
        shared.v3(tf.translation);
        shared.quat(tf.rotation);
        shared.f32(v.turret_yaw);
        shared.f32(v.turret_pitch);
        shared.f32(v.rotor);
        shared.f32(h.hp / h.max);
        shared.f32(v.speed);
        shared.u32(netid(v.driver));
        shared.u8(v.passengers.len() as u8);
    }
    shared.u8(sectors.list.len() as u8);
    for s in &sectors.list {
        shared.u8(s.owner as u8);
        shared.u8((s.capture.clamp(0.0, 1.0) * 255.0) as u8);
    }
    let mut ev = W::default();
    let pending: Vec<&(u32, NetEvent, u8)> = host.events.iter().take(600).collect();
    ev.u16(pending.len() as u16);
    for (seq, e, _) in &pending {
        write_event(&mut ev, *seq, e);
    }
    for e in host.events.iter_mut().take(600) {
        e.2 = e.2.saturating_sub(1);
    }
    host.events.retain(|e| e.2 > 0);

    for c in &host.clients {
        let Some(slot) = humans.get(c.id) else { continue };
        let mut st = private_state(slot, &soldiers_full, &vehicles_full, &squads, &terrain);
        st.soldier_id = netid(slot.soldier);
        for (k, m) in slot_squad_ids(slot, &squads).into_iter().enumerate() {
            if let Some(entry) = st.squad.get_mut(k) {
                entry.3 = netid(Some(m));
            }
        }
        let mut w = W::default();
        w.u8(S_SNAPSHOT);
        w.u32(host.tick);
        write_private(&mut w, &st);
        w.0.extend_from_slice(&shared.0);
        w.0.extend_from_slice(&ev.0);
        if w.0.len() <= MAX_PACKET {
            let _ = host.socket.send_to(&w.0, c.addr);
        }
    }
}

fn slot_squad_ids(slot: &HumanSlot, squads: &Query<&Squad>) -> Vec<Entity> {
    slot.squad
        .and_then(|s| squads.get(s).ok())
        .map(|sq| sq.members.clone())
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn client_receive(
    mut commands: Commands,
    mut net: ResMut<NetMode>,
    time: Res<Time>,
    mut index: ResMut<NetIndex>,
    mut local: ResMut<LocalState>,
    soldier_assets: Res<SoldierAssets>,
    vehicle_assets: Res<VehicleAssets>,
    mut soldiers: Query<(&mut Soldier, &mut Health, &mut Interp), Without<Vehicle>>,
    mut vehicles: Query<(&mut Vehicle, &mut Health, &mut Interp), Without<Soldier>>,
    mut sectors: ResMut<Sectors>,
    mut shots: MessageWriter<ShotFx>,
    mut explosions: MessageWriter<ExplosionFx>,
    mut sounds: MessageWriter<PlaySound>,
    mut radio: MessageWriter<Radio>,
    mut trees: MessageWriter<TreeFell>,
) {
    let NetMode::Client(client) = &mut *net else { return };
    let now = time.elapsed_secs_f64();
    let mut buf = vec![0u8; MAX_PACKET + 1024];
    let mut latest: Option<Vec<u8>> = None;
    loop {
        let n = match client.socket.recv_from(&mut buf) {
            Ok((n, from)) if from == client.server => n,
            Ok(_) => continue,
            Err(_) => break,
        };
        let mut r = R::new(&buf[..n]);
        match r.u8() {
            Some(S_WELCOME) => {
                if let Some(id) = r.u32() {
                    if client.id.is_none() {
                        info!("Joined server as player {id}");
                    }
                    client.id = Some(id);
                    local.id = id;
                }
            }
            Some(S_SNAPSHOT) => {
                let Some(tick) = r.u32() else { continue };
                // Every packet carries events; only the newest one's state matters.
                let mut rr = R::new(&buf[5..n]);
                let Some(_) = read_private(&mut rr) else { continue };
                if skip_entities(&mut rr).is_none() {
                    continue;
                }
                if let Some(count) = rr.u16() {
                    for _ in 0..count {
                        let Some((seq, e)) = read_event(&mut rr) else { break };
                        if !client.seen.insert(seq) {
                            continue;
                        }
                        client.seen_order.push_back(seq);
                        if client.seen_order.len() > 4096
                            && let Some(old) = client.seen_order.pop_front()
                        {
                            client.seen.remove(&old);
                        }
                        match e {
                            NetEvent::Shot(s) => {
                                shots.write(s);
                            }
                            NetEvent::Explosion(x) => {
                                explosions.write(x);
                            }
                            NetEvent::Sound(s) => {
                                sounds.write(s);
                            }
                            NetEvent::Radio(m) => {
                                radio.write(m);
                            }
                            NetEvent::Tree(t) => {
                                trees.write(t);
                            }
                        }
                    }
                }
                if tick > client.last_tick {
                    client.last_tick = tick;
                    latest = Some(buf[5..n].to_vec());
                }
            }
            _ => {}
        }
    }
    let Some(data) = latest else { return };
    client.last_snapshot = now;
    let mut r = R::new(&data);
    let Some(private) = read_private(&mut r) else { return };

    let mut present: HashSet<u32> = HashSet::new();
    let Some(n) = r.u16() else { return };
    let mut soldier_vehicle: Vec<(Entity, u32)> = Vec::new();
    for _ in 0..n {
        let Some(row) = (|| {
            Some((
                r.u32()?,
                r.u8()?,
                r.u8()?,
                r.u8()?,
                r.u8()?,
                r.u8()?,
                r.v3()?,
                r.f32()?,
                r.f32()?,
                r.u32()?,
                r.str()?,
            ))
        })() else {
            return;
        };
        let (id, faction, role, stance, flags, held, pos, yaw, pitch, veh, name) = row;
        present.insert(id);
        let faction = if faction == 0 {
            Faction::Blufor
        } else {
            Faction::Opfor
        };
        let e = match index.map.get(&id) {
            Some(&e) => e,
            None => {
                let e = spawn_soldier(
                    &mut commands,
                    &soldier_assets,
                    name,
                    faction,
                    Role::from_u8(role),
                    pos,
                    yaw,
                );
                commands.entity(e).insert((
                    NetId(id),
                    Interp {
                        from: pos,
                        to: pos,
                        from_rot: Quat::from_rotation_y(yaw),
                        to_rot: Quat::from_rotation_y(yaw),
                        t: 1.0,
                    },
                ));
                index.map.insert(id, e);
                continue;
            }
        };
        if let Ok((mut s, mut h, mut interp)) = soldiers.get_mut(e) {
            s.stance = Stance::from_u8(stance);
            s.velocity = (pos - interp.to) / SNAPSHOT_INTERVAL;
            s.velocity.y = 0.0;
            if s.velocity.length() > 15.0 {
                s.velocity = Vec3::ZERO;
            }
            s.walk_phase += s.velocity.length() * SNAPSHOT_INTERVAL * 2.2;
            s.yaw = yaw;
            s.pitch = pitch;
            s.held = WeaponKind::from_u8(held);
            s.sprinting = flags & 2 != 0;
            h.dead = flags & 1 != 0;
            soldier_vehicle.push((e, veh));
            interp.from = interp.from.lerp(interp.to, interp.t.min(1.0));
            interp.from_rot = interp.from_rot.slerp(interp.to_rot, interp.t.min(1.0));
            interp.to = pos;
            interp.to_rot = Quat::from_rotation_y(yaw);
            interp.t = 0.0;
        }
    }
    let Some(n) = r.u16() else { return };
    for _ in 0..n {
        let Some(row) = (|| {
            Some((
                r.u32()?,
                r.u8()?,
                r.u8()?,
                r.u8()?,
                r.v3()?,
                r.quat()?,
                r.f32()?,
                r.f32()?,
                r.f32()?,
                r.f32()?,
                r.f32()?,
                r.u32()?,
                r.u8()?,
            ))
        })() else {
            return;
        };
        let (id, kind, paint, flags, pos, rot, ty, tp, rotor, hp, speed, driver, passengers) = row;
        present.insert(id);
        let e = match index.map.get(&id) {
            Some(&e) => e,
            None => {
                let paint = if paint == 0 {
                    Faction::Blufor
                } else {
                    Faction::Opfor
                };
                let (yaw, _, _) = rot.to_euler(EulerRot::YXZ);
                let e = spawn_vehicle(
                    &mut commands,
                    &vehicle_assets,
                    VehicleKind::from_u8(kind),
                    paint,
                    pos,
                    yaw,
                );
                commands.entity(e).insert((
                    NetId(id),
                    Interp {
                        from: pos,
                        to: pos,
                        from_rot: rot,
                        to_rot: rot,
                        t: 1.0,
                    },
                ));
                index.map.insert(id, e);
                continue;
            }
        };
        if let Ok((mut v, mut h, mut interp)) = vehicles.get_mut(e) {
            v.destroyed = flags & 1 != 0;
            v.crew_faction = match flags >> 1 {
                1 => Some(Faction::Blufor),
                2 => Some(Faction::Opfor),
                _ => None,
            };
            v.velocity = (pos - interp.to) / SNAPSHOT_INTERVAL;
            v.turret_yaw = ty;
            v.turret_pitch = tp;
            v.rotor = rotor;
            v.speed = speed;
            h.hp = hp * h.max;
            v.driver = index.map.get(&driver).copied();
            v.passengers = vec![e; passengers as usize];
            interp.from = interp.from.lerp(interp.to, interp.t.min(1.0));
            interp.from_rot = interp.from_rot.slerp(interp.to_rot, interp.t.min(1.0));
            interp.to = pos;
            interp.to_rot = rot;
            interp.t = 0.0;
        }
    }
    for (e, veh) in soldier_vehicle {
        let ve = index.map.get(&veh).copied();
        if let Ok((mut s, _, _)) = soldiers.get_mut(e) {
            s.vehicle = ve;
        }
    }
    if let Some(n) = r.u8() {
        for i in 0..n as usize {
            let (Some(owner), Some(cap)) = (r.u8(), r.u8()) else { break };
            if let Some(s) = sectors.list.get_mut(i) {
                s.owner = if owner == 0 {
                    Faction::Blufor
                } else {
                    Faction::Opfor
                };
                s.capture = cap as f32 / 255.0;
            }
        }
    }
    // Anything the host no longer sends is gone.
    let gone: Vec<u32> = index
        .map
        .keys()
        .copied()
        .filter(|id| !present.contains(id))
        .collect();
    for id in gone {
        if let Some(e) = index.map.remove(&id) {
            commands.entity(e).despawn();
        }
    }
    local.soldier = index.map.get(&private.soldier_id).copied();
    local.state = private;
}

/// Skip over the entity and sector tables to reach the event list.
fn skip_entities(r: &mut R) -> Option<()> {
    let n = r.u16()?;
    for _ in 0..n {
        r.take(4 + 5)?;
        r.take(12 + 8 + 4)?;
        r.str()?;
    }
    let n = r.u16()?;
    for _ in 0..n {
        r.take(4 + 3 + 12 + 16 + 20 + 4 + 1)?;
    }
    let n = r.u8()?;
    r.take(n as usize * 2)?;
    Some(())
}

fn client_send(mut net: ResMut<NetMode>, time: Res<Time>, input: Res<LocalInput>) {
    let NetMode::Client(client) = &mut *net else { return };
    let dt = time.delta_secs();
    if client.id.is_none() {
        client.hello_timer -= dt;
        if client.hello_timer <= 0.0 {
            client.hello_timer = 0.5;
            let mut w = W::default();
            w.u8(C_HELLO);
            w.str(&client.name);
            let _ = client.socket.send_to(&w.0, client.server);
        }
        return;
    }
    client.send_timer -= dt;
    if client.send_timer > 0.0 {
        return;
    }
    client.send_timer = 1.0 / 60.0;
    let mut w = W::default();
    w.u8(C_INPUT);
    write_input(&mut w, &input.0);
    let _ = client.socket.send_to(&w.0, client.server);
}

fn client_interpolate(time: Res<Time>, mut q: Query<(&mut Interp, &mut Transform)>) {
    let step = time.delta_secs() / SNAPSHOT_INTERVAL;
    for (mut i, mut tf) in &mut q {
        i.t = (i.t + step).min(1.25);
        let t = i.t.min(1.0);
        tf.translation = i.from.lerp(i.to, t);
        tf.rotation = i.from_rot.slerp(i.to_rot, t);
    }
}

fn say_goodbye(net: Res<NetMode>, mut exits: MessageReader<AppExit>) {
    let NetMode::Client(client) = &*net else { return };
    if exits.read().next().is_some() {
        let _ = client.socket.send_to(&[C_BYE], client.server);
    }
}

