//! Human players: input capture, applying inputs on the host, respawning, camera and view model.

use std::f32::consts::FRAC_PI_2;

use bevy::{
    camera::visibility::RenderLayers,
    input::mouse::AccumulatedMouseMotion,
    light::NotShadowCaster,
    pbr::{DistanceFog, FogFalloff},
    prelude::*,
    window::{CursorGrabMode, CursorOptions},
};

use crate::ai::{AiBrain, Order, Squad, SquadCommand};
use crate::combat::WeaponKind;
use crate::common::*;
use crate::net::is_authority;
use crate::soldier::{Loadout, Role, Soldier, SoldierAssets, Stance, random_name, spawn_soldier};
use crate::terrain::{BASE, LOCATIONS, Terrain};
use crate::vehicle::{EnterVehicle, ExitVehicle, Vehicle, VehicleKind, drop_compensation};
use crate::world::{BULLET, Obstacles};

// Edge-triggered actions are counters so a lost network packet never loses a key press.
pub const E_FIRE: usize = 0;
pub const E_RELOAD: usize = 1;
pub const E_CROUCH: usize = 2;
pub const E_PRONE: usize = 3;
pub const E_GRENADE: usize = 4;
pub const E_HEAL: usize = 5;
pub const E_INTERACT: usize = 6;
pub const E_FIREMODE: usize = 7;
pub const E_WEAPON1: usize = 8;
pub const E_WEAPON2: usize = 9;
pub const E_NEXT_WEAPON: usize = 10;
pub const E_RESPAWN: usize = 11;
pub const E_CMD_FOLLOW: usize = 12;
pub const E_CMD_HOLD: usize = 13;
pub const E_CMD_MOVE: usize = 14;
pub const EDGE_COUNT: usize = 15;

#[derive(Clone, Default, Debug)]
pub struct PlayerInput {
    pub move_axis: Vec2,
    pub yaw: f32,
    pub pitch: f32,
    pub sprint: bool,
    pub fire: bool,
    pub ads: bool,
    pub up: bool,
    pub down: bool,
    pub roll_left: bool,
    pub roll_right: bool,
    pub brake: bool,
    pub aim_point: Vec3,
    pub zeroing: u16,
    pub role: u8,
    pub edges: [u8; EDGE_COUNT],
}

impl PlayerInput {
    pub fn bump(&mut self, edge: usize) {
        self.edges[edge] = self.edges[edge].wrapping_add(1);
    }
}

pub struct HumanSlot {
    pub id: u32,
    pub name: String,
    pub soldier: Option<Entity>,
    pub squad: Option<Entity>,
    pub input: PlayerInput,
    pub seen: [u8; EDGE_COUNT],
    pub respawn_timer: f32,
    pub hurt_count: u32,
    pub hit_count: u32,
    pub kills: u32,
    pub deaths: u32,
    pub connected: bool,
}

#[derive(Resource, Default)]
pub struct Humans {
    pub slots: Vec<HumanSlot>,
}

impl Humans {
    pub fn get(&self, id: u32) -> Option<&HumanSlot> {
        self.slots.iter().find(|s| s.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut HumanSlot> {
        self.slots.iter_mut().find(|s| s.id == id)
    }

    pub fn add(&mut self, id: u32, name: String) -> &mut HumanSlot {
        self.slots.push(HumanSlot {
            id,
            name,
            soldier: None,
            squad: None,
            input: PlayerInput {
                zeroing: 200,
                ..default()
            },
            seen: [0; EDGE_COUNT],
            respawn_timer: 0.0,
            hurt_count: 0,
            hit_count: 0,
            kills: 0,
            deaths: 0,
            connected: true,
        });
        self.slots.last_mut().unwrap()
    }
}

#[derive(Component)]
pub struct Human {
    pub id: u32,
}

/// Everything one player's HUD needs; built on the host and sent to that player.
#[derive(Clone, Default, Debug)]
pub struct PrivateState {
    pub soldier_id: u32,
    pub alive: bool,
    pub hp: f32,
    pub stamina: f32,
    pub stance: u8,
    pub weapons: Vec<u8>,
    pub active: u8,
    pub ammo: u32,
    pub mags: u32,
    pub reloading: bool,
    pub auto_mode: bool,
    pub grenades: u32,
    pub fak: u32,
    pub healing: bool,
    pub shots: u32,
    pub hurt: u32,
    pub hit: u32,
    pub respawn_timer: f32,
    pub kills: u32,
    pub deaths: u32,
    pub in_vehicle: bool,
    pub driver: bool,
    pub vehicle_kind: u8,
    pub vehicle_paint: u8,
    pub vehicle_hp: f32,
    pub vehicle_speed: f32,
    pub vehicle_alt: f32,
    pub vehicle_weapon: u8,
    pub vehicle_ammo: u32,
    pub vehicle_mags: u32,
    pub vehicle_reloading: bool,
    pub squad_name: String,
    pub order: String,
    /// Name, role, alive, replication id.
    pub squad: Vec<(String, u8, bool, u32)>,
}

/// The local player's state (from the host's world, or from the network on clients).
#[derive(Resource, Default)]
pub struct LocalState {
    pub state: PrivateState,
    pub soldier: Option<Entity>,
    pub id: u32,
}

/// Local-only view state (camera angles, toggles).
#[derive(Resource)]
pub struct LocalView {
    pub yaw: f32,
    pub pitch: f32,
    pub look_yaw: f32,
    pub look_pitch: f32,
    pub third_person: bool,
    pub map_open: bool,
    pub ads: f32,
    pub dead: bool,
    pub zeroing: u16,
    pub role: u8,
    pub aim_point: Vec3,
    pub last_shots: u32,
    pub last_hurt: u32,
    pub last_hit: u32,
    pub fov: f32,
    pub initialized: bool,
    pub scoreboard: bool,
}

impl Default for LocalView {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            look_yaw: 0.0,
            look_pitch: 0.0,
            third_person: false,
            map_open: false,
            ads: 0.0,
            dead: false,
            zeroing: 200,
            role: 0,
            aim_point: Vec3::ZERO,
            last_shots: 0,
            last_hurt: 0,
            last_hit: 0,
            fov: 75f32.to_radians(),
            initialized: false,
            scoreboard: false,
        }
    }
}

#[derive(Resource, Default)]
pub struct LocalInput(pub PlayerInput);

#[derive(Component)]
pub struct MainCamera;

#[derive(Component)]
pub struct ViewModel;

#[derive(Component)]
pub struct ViewModelPart(pub ViewGun);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewGun {
    Rifle,
    Mg,
    Launcher,
}

pub const VIEW_LAYER: usize = 1;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Humans>()
            .init_resource::<LocalState>()
            .init_resource::<LocalView>()
            .init_resource::<LocalInput>()
            .init_resource::<PlayerFeedback>()
            .add_systems(Startup, spawn_cameras)
            .add_systems(
                Update,
                (cursor_grab, read_local_input)
                    .chain()
                    .in_set(Phase::Input),
            )
            .add_systems(
                Update,
                (copy_local_input, respawn_humans, apply_human_inputs)
                    .chain()
                    .after(read_local_input)
                    .in_set(Phase::Input)
                    .run_if(is_authority),
            )
            .add_systems(
                Update,
                (human_deaths, build_local_state)
                    .chain()
                    .after(crate::combat::apply_damage)
                    .in_set(Phase::Damage)
                    .run_if(is_authority),
            )
            .add_systems(
                Update,
                (mark_local_player, local_feedback, update_camera, update_view_model)
                    .chain()
                    .in_set(Phase::Camera),
            );
    }
}

fn spawn_cameras(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let cam = commands
        .spawn((
            MainCamera,
            Camera3d::default(),
            Projection::from(PerspectiveProjection {
                fov: 75f32.to_radians(),
                near: 0.05,
                far: 4000.0,
                ..default()
            }),
            DistanceFog {
                color: SKY_COLOR,
                directional_light_color: Color::srgba(1.0, 0.95, 0.85, 0.5),
                directional_light_exponent: 30.0,
                falloff: FogFalloff::Linear {
                    start: 250.0,
                    end: 1900.0,
                },
            },
            Transform::from_xyz(0.0, 100.0, 0.0),
            bevy::audio::SpatialListener::new(0.3),
        ))
        .id();
    let view_cam = commands
        .spawn((
            Camera3d::default(),
            Camera {
                order: 1,
                ..default()
            },
            Projection::from(PerspectiveProjection {
                fov: 60f32.to_radians(),
                near: 0.01,
                far: 10.0,
                ..default()
            }),
            RenderLayers::layer(VIEW_LAYER),
        ))
        .id();

    // First-person weapons, rendered by the view-model camera on top of the world.
    let mut m = |c: Color, rough: f32, metallic: f32| {
        materials.add(StandardMaterial {
            base_color: c,
            perceptual_roughness: rough,
            metallic,
            ..default()
        })
    };
    let black = m(Color::srgb(0.06, 0.06, 0.06), 0.45, 0.4);
    let tan = m(Color::srgb(0.55, 0.48, 0.36), 0.8, 0.0);
    let olive = m(Color::srgb(0.3, 0.33, 0.22), 0.7, 0.0);
    let glove = m(Color::srgb(0.22, 0.2, 0.17), 0.9, 0.0);
    let dot = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.1, 0.05),
        emissive: LinearRgba::rgb(40.0, 1.0, 0.5),
        unlit: true,
        ..default()
    });
    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let cyl = meshes.add(Cylinder::new(1.0, 1.0));
    let ring = meshes.add(Torus::new(0.016, 0.02));
    let dot_mesh = meshes.add(Sphere::new(0.0016));
    let layer = RenderLayers::layer(VIEW_LAYER);
    let root = commands
        .spawn((ViewModel, Transform::from_scale(Vec3::splat(0.8)), Visibility::default()))
        .id();
    let piece = |commands: &mut Commands,
                     gun: ViewGun,
                     mesh: &Handle<Mesh>,
                     mat: &Handle<StandardMaterial>,
                     pos: Vec3,
                     scale: Vec3,
                     rot: Quat| {
        let e = commands
            .spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat.clone()),
                Transform {
                    translation: pos,
                    rotation: rot,
                    scale,
                },
                ViewModelPart(gun),
                layer.clone(),
                NotShadowCaster,
            ))
            .id();
        commands.entity(root).add_child(e);
    };
    let along_z = Quat::from_rotation_x(FRAC_PI_2);
    let ring_rot = Quat::from_rotation_x(FRAC_PI_2);
    // The sight line is the model's Z axis at y = 0; the bore sits below it.
    for (gun, body, len) in [(ViewGun::Rifle, &black, 0.62), (ViewGun::Mg, &black, 0.75)] {
        piece(&mut commands, gun, &cube, body, Vec3::new(0.0, -0.075, 0.05), Vec3::new(0.05, 0.07, 0.42), Quat::IDENTITY);
        piece(&mut commands, gun, &cube, &tan, Vec3::new(0.0, -0.08, -0.25), Vec3::new(0.06, 0.06, 0.25), Quat::IDENTITY);
        piece(&mut commands, gun, &cyl, body, Vec3::new(0.0, -0.07, -len * 0.5 - 0.2), Vec3::new(0.012, len, 0.012), along_z);
        piece(&mut commands, gun, &cube, body, Vec3::new(0.0, -0.06, 0.3), Vec3::new(0.045, 0.09, 0.2), Quat::IDENTITY);
        piece(&mut commands, gun, &cube, &glove, Vec3::new(0.0, -0.15, 0.12), Vec3::new(0.05, 0.1, 0.05), Quat::from_rotation_x(0.3));
        piece(&mut commands, gun, &cube, &glove, Vec3::new(-0.02, -0.12, -0.22), Vec3::new(0.06, 0.05, 0.12), Quat::IDENTITY);
        piece(&mut commands, gun, &ring, body, Vec3::new(0.0, 0.0, -0.02), Vec3::ONE, ring_rot);
        piece(&mut commands, gun, &ring, body, Vec3::new(0.0, 0.0, -0.1), Vec3::ONE, ring_rot);
        piece(&mut commands, gun, &cube, body, Vec3::new(0.0, -0.03, -0.06), Vec3::new(0.02, 0.025, 0.1), Quat::IDENTITY);
        piece(&mut commands, gun, &dot_mesh, &dot, Vec3::new(0.0, 0.0, -0.1), Vec3::ONE, Quat::IDENTITY);
        if gun == ViewGun::Mg {
            piece(&mut commands, gun, &cube, &olive, Vec3::new(0.0, -0.15, -0.05), Vec3::new(0.09, 0.1, 0.12), Quat::IDENTITY);
        } else {
            piece(&mut commands, gun, &cube, body, Vec3::new(0.0, -0.15, -0.03), Vec3::new(0.035, 0.12, 0.05), Quat::from_rotation_x(-0.2));
        }
    }
    piece(&mut commands, ViewGun::Launcher, &cyl, &olive, Vec3::new(0.0, -0.08, -0.15), Vec3::new(0.05, 1.0, 0.05), along_z);
    piece(&mut commands, ViewGun::Launcher, &ring, &black, Vec3::new(0.0, 0.0, -0.12), Vec3::ONE, ring_rot);
    piece(&mut commands, ViewGun::Launcher, &dot_mesh, &dot, Vec3::new(0.0, 0.0, -0.12), Vec3::ONE, Quat::IDENTITY);
    piece(&mut commands, ViewGun::Launcher, &cube, &glove, Vec3::new(0.0, -0.17, 0.0), Vec3::new(0.05, 0.1, 0.05), Quat::IDENTITY);
    commands.entity(cam).add_children(&[view_cam, root]);
}

fn cursor_grab(
    mut cursor: Single<&mut CursorOptions>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if mouse.just_pressed(MouseButton::Left) && cursor.grab_mode == CursorGrabMode::None {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

#[allow(clippy::too_many_arguments)]
pub fn read_local_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    cursor: Single<&CursorOptions>,
    mut view: ResMut<LocalView>,
    mut input: ResMut<LocalInput>,
    local: Res<LocalState>,
) {
    let locked = cursor.grab_mode != CursorGrabMode::None;
    let inp = &mut input.0;
    let st = &local.state;
    if keys.just_pressed(KeyCode::KeyM) {
        view.map_open = !view.map_open;
    }
    view.scoreboard = keys.pressed(KeyCode::Tab);
    if keys.just_pressed(KeyCode::KeyV) {
        view.third_person = !view.third_person;
    }
    if keys.just_pressed(KeyCode::PageUp) {
        view.zeroing = (view.zeroing + 100).min(800);
    }
    if keys.just_pressed(KeyCode::PageDown) {
        view.zeroing = view.zeroing.saturating_sub(100).max(100);
    }
    if !st.alive {
        for (k, r) in [(KeyCode::Digit1, 0u8), (KeyCode::Digit2, 1), (KeyCode::Digit3, 2), (KeyCode::Digit4, 3)] {
            if keys.just_pressed(k) {
                view.role = r;
            }
        }
    }
    let freelook = keys.pressed(KeyCode::AltLeft) && !st.in_vehicle;
    if locked && !view.map_open {
        let zoom = view.fov / 75f32.to_radians();
        let delta = motion.delta * 0.0022 * zoom;
        if freelook {
            view.look_yaw = (view.look_yaw - delta.x).clamp(-2.0, 2.0);
            view.look_pitch = (view.look_pitch - delta.y).clamp(-1.2, 1.2);
        } else {
            view.yaw -= delta.x;
            view.pitch = (view.pitch - delta.y).clamp(-1.5, 1.5);
        }
    }
    if !freelook {
        let k = 0.85f32;
        view.look_yaw *= k;
        view.look_pitch *= k;
    }
    let active = locked && !view.map_open;
    let axis = |pos: KeyCode, neg: KeyCode| {
        if !active {
            return 0.0;
        }
        (keys.pressed(pos) as i32 - keys.pressed(neg) as i32) as f32
    };
    inp.move_axis = Vec2::new(axis(KeyCode::KeyD, KeyCode::KeyA), axis(KeyCode::KeyW, KeyCode::KeyS));
    inp.yaw = view.yaw;
    inp.pitch = view.pitch;
    inp.sprint = active && keys.pressed(KeyCode::ShiftLeft);
    inp.up = active && keys.pressed(KeyCode::ShiftLeft);
    inp.down = active && keys.pressed(KeyCode::ControlLeft);
    inp.roll_left = active && keys.pressed(KeyCode::KeyQ);
    inp.roll_right = active && keys.pressed(KeyCode::KeyE);
    inp.brake = active && keys.pressed(KeyCode::Space);
    inp.fire = active && mouse.pressed(MouseButton::Left);
    inp.ads = active && mouse.pressed(MouseButton::Right);
    inp.aim_point = view.aim_point;
    inp.zeroing = view.zeroing;
    inp.role = view.role;
    if !active {
        return;
    }
    if mouse.just_pressed(MouseButton::Left) {
        inp.bump(E_FIRE);
    }
    let edges = [
        (KeyCode::KeyR, E_RELOAD),
        (KeyCode::KeyC, E_CROUCH),
        (KeyCode::KeyZ, E_PRONE),
        (KeyCode::KeyG, E_GRENADE),
        (KeyCode::KeyH, E_HEAL),
        (KeyCode::KeyF, E_INTERACT),
        (KeyCode::KeyB, E_FIREMODE),
        (KeyCode::Digit1, E_WEAPON1),
        (KeyCode::Digit2, E_WEAPON2),
        (KeyCode::KeyX, E_NEXT_WEAPON),
        (KeyCode::Enter, E_RESPAWN),
        (KeyCode::F1, E_CMD_FOLLOW),
        (KeyCode::F2, E_CMD_HOLD),
        (KeyCode::F3, E_CMD_MOVE),
    ];
    for (key, edge) in edges {
        if keys.just_pressed(key) {
            inp.bump(edge);
        }
    }
}

pub fn copy_local_input(local: Res<LocalInput>, mut humans: ResMut<Humans>, state: Res<LocalState>) {
    if let Some(slot) = humans.get_mut(state.id) {
        slot.input = local.0.clone();
    }
}

fn respawn_point(index: u32) -> Vec3 {
    let base = LOCATIONS[BASE].pos;
    let a = index as f32 * 1.7;
    Vec3::new(base.x - 20.0 + a.cos() * 6.0, 0.0, base.y + 10.0 + a.sin() * 6.0)
}

/// Spawn (or respawn) each human's soldier at the FOB, with a fresh AI fireteam the first time.
fn respawn_humans(
    mut commands: Commands,
    time: Res<Time>,
    mut humans: ResMut<Humans>,
    assets: Res<SoldierAssets>,
    terrain: Res<Terrain>,
    soldiers: Query<&Health, With<Soldier>>,
    squads: Query<&Squad>,
    mut radio: MessageWriter<Radio>,
) {
    let dt = time.delta_secs();
    for slot in humans.slots.iter_mut() {
        if !slot.connected {
            continue;
        }
        let alive = slot
            .soldier
            .and_then(|s| soldiers.get(s).ok())
            .is_some_and(|h| !h.dead);
        if alive {
            continue;
        }
        slot.respawn_timer -= dt;
        let first = slot.soldier.is_none() && slot.deaths == 0;
        let wants = slot.input.edges[E_RESPAWN] != slot.seen[E_RESPAWN];
        slot.seen[E_RESPAWN] = slot.input.edges[E_RESPAWN];
        if !(first || (wants && slot.respawn_timer <= 0.0)) {
            continue;
        }
        let role = match slot.input.role {
            1 => Role::Autorifleman,
            2 => Role::AtSoldier,
            3 => Role::Rifleman,
            _ => Role::Leader,
        };
        let mut pos = respawn_point(slot.id);
        pos.y = terrain.height(pos.x, pos.z);
        let rank = match role {
            Role::Leader => "Sgt.",
            Role::Rifleman => "Pvt.",
            _ => "Cpl.",
        };
        let e = spawn_soldier(
            &mut commands,
            &assets,
            format!("{rank} {}", slot.name),
            Faction::Blufor,
            role,
            pos,
            -0.9,
        );
        commands.entity(e).insert(Human { id: slot.id });
        // Each human leads a fireteam; it survives their deaths.
        let squad = match slot.squad.filter(|&s| squads.get(s).is_ok()) {
            Some(s) => s,
            None => {
                let names = ["Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot"];
                let name = format!("{} 1-{}", names[slot.id as usize % names.len()], slot.id + 1);
                let mut sq = Squad::new(name, Faction::Blufor, Order::Follow);
                sq.human_led = true;
                let sq_e = commands.spawn((sq, MissionEntity)).id();
                for (i, r) in [Role::Rifleman, Role::Autorifleman, Role::AtSoldier]
                    .into_iter()
                    .enumerate()
                {
                    let off = Vec3::new(3.0 + i as f32 * 2.0, 0.0, 3.0);
                    let mut p = pos + off;
                    p.y = terrain.height(p.x, p.z);
                    let m = spawn_soldier(
                        &mut commands,
                        &assets,
                        random_name(Faction::Blufor, r),
                        Faction::Blufor,
                        r,
                        p,
                        -0.9,
                    );
                    commands.entity(m).insert(AiBrain::new(0.7));
                    commands.queue(move |world: &mut World| {
                        if let Some(mut s) = world.get_mut::<Soldier>(m) {
                            s.squad = Some(sq_e);
                        }
                        if let Some(mut q) = world.get_mut::<Squad>(sq_e) {
                            q.members.push(m);
                        }
                    });
                }
                slot.squad = Some(sq_e);
                sq_e
            }
        };
        let old = slot.soldier;
        commands.queue(move |world: &mut World| {
            if let Some(mut s) = world.get_mut::<Soldier>(e) {
                s.squad = Some(squad);
            }
            if let Some(mut q) = world.get_mut::<Squad>(squad) {
                q.members.retain(|&m| Some(m) != old);
                q.members.insert(0, e);
                q.order = Order::Follow;
            }
        });
        if let Some(old) = old {
            commands.entity(old).remove::<Human>();
        }
        slot.soldier = Some(e);
        slot.respawn_timer = 0.0;
        if !first {
            radio.write(Radio::new(
                format!("{} redeployed at {}", slot.name, LOCATIONS[BASE].name),
                RADIO_SIDE,
            ));
        }
    }
}

#[allow(clippy::type_complexity)]
pub fn apply_human_inputs(
    mut humans: ResMut<Humans>,
    mut soldiers: Query<(&mut Soldier, &Loadout, &Health, &Transform)>,
    mut vehicles: Query<&mut Vehicle>,
    mut enters: MessageWriter<EnterVehicle>,
    mut exits: MessageWriter<ExitVehicle>,
    mut orders: MessageWriter<SquadCommand>,
    mut radio: MessageWriter<Radio>,
) {
    for slot in humans.slots.iter_mut() {
        let Some(se) = slot.soldier else { continue };
        let Ok((mut s, loadout, health, stf)) = soldiers.get_mut(se) else {
            continue;
        };
        let inp = slot.input.clone();
        let mut pressed = [false; EDGE_COUNT];
        for (i, p) in pressed.iter_mut().enumerate() {
            *p = inp.edges[i] != slot.seen[i];
            slot.seen[i] = inp.edges[i];
        }
        if health.dead {
            continue;
        }
        if (pressed[E_CMD_FOLLOW] || pressed[E_CMD_HOLD] || pressed[E_CMD_MOVE])
            && let Some(sq) = slot.squad
        {
            let (order, text) = if pressed[E_CMD_FOLLOW] {
                (Order::Follow, "All units, form on me.".to_string())
            } else if pressed[E_CMD_HOLD] {
                (Order::Hold(stf.translation), "Hold position.".to_string())
            } else {
                (
                    Order::Move(inp.aim_point),
                    format!("Move to grid {}.", grid_ref(inp.aim_point)),
                )
            };
            orders.write(SquadCommand { squad: sq, order });
            radio.write(Radio::new(format!("{}: {}", s.name, text), RADIO_GROUP));
        }

        if let Some(ve) = s.vehicle {
            if pressed[E_INTERACT] {
                exits.write(ExitVehicle { soldier: se });
            }
            let Ok(mut v) = vehicles.get_mut(ve) else { continue };
            if v.driver != Some(se) {
                continue;
            }
            let axis = inp.move_axis;
            v.input.throttle = axis.y;
            v.input.steer = axis.x;
            v.input.brake = inp.brake;
            v.input.collective = inp.up as i32 as f32 - inp.down as i32 as f32;
            v.input.cyclic_pitch = axis.y;
            v.input.pedal = axis.x;
            v.input.cyclic_roll = inp.roll_right as i32 as f32 - inp.roll_left as i32 as f32;
            v.input.fire = inp.fire;
            v.input.aim_point = Some(inp.aim_point);
            if pressed[E_NEXT_WEAPON] || pressed[E_WEAPON1] || pressed[E_WEAPON2] {
                v.input.switch_weapon = true;
            }
            if v.kind == VehicleKind::Heli {
                v.input.throttle = 0.0;
            }
            continue;
        }

        let kind = loadout.current().kind;
        let zero = drop_compensation(kind, inp.zeroing as f32);
        s.yaw = inp.yaw;
        s.pitch = inp.pitch + zero;
        let fwd = forward_of_yaw(inp.yaw);
        let right = right_of_yaw(inp.yaw);
        let mut dir = fwd * inp.move_axis.y + right * inp.move_axis.x;
        if dir.length_squared() > 1.0 {
            dir = dir.normalize();
        }
        let stance = s.stance;
        let intent = &mut s.intent;
        intent.move_dir = dir;
        intent.sprint = inp.sprint;
        intent.ads = inp.ads;
        let auto = loadout.current().auto_mode;
        intent.fire = if auto { inp.fire } else { pressed[E_FIRE] };
        if kind == WeaponKind::Launcher && !inp.ads {
            intent.fire = false;
        }
        intent.reload |= pressed[E_RELOAD];
        intent.throw |= pressed[E_GRENADE];
        intent.heal |= pressed[E_HEAL];
        intent.toggle_mode |= pressed[E_FIREMODE];
        if pressed[E_WEAPON1] {
            intent.switch_to = Some(0);
        }
        if pressed[E_WEAPON2] {
            intent.switch_to = Some(1);
        }
        if pressed[E_NEXT_WEAPON] {
            intent.switch_to = Some((loadout.active + 1) % loadout.weapons.len());
        }
        if pressed[E_CROUCH] {
            intent.stance = Some(if stance == Stance::Crouch {
                Stance::Stand
            } else {
                Stance::Crouch
            });
        }
        if pressed[E_PRONE] {
            intent.stance = Some(if stance == Stance::Prone {
                Stance::Stand
            } else {
                Stance::Prone
            });
        }
        if pressed[E_INTERACT] {
            enters.write(EnterVehicle {
                soldier: se,
                vehicle: None,
            });
        }
    }
}

fn human_deaths(
    mut events: MessageReader<Killed>,
    mut humans: ResMut<Humans>,
    victims: Query<(&Human, &Soldier)>,
    attackers: Query<(&Human, &Soldier)>,
    vehicle_drivers: Query<&Vehicle>,
) {
    let _ = vehicle_drivers;
    for ev in events.read() {
        let victim_faction = victims.get(ev.victim).ok().map(|(h, s)| (h.id, s.faction));
        if let Some((id, _)) = victim_faction
            && let Some(slot) = humans.get_mut(id)
        {
            slot.deaths += 1;
            slot.respawn_timer = 8.0;
        }
        if let Some(a) = ev.attacker
            && a != ev.victim
            && let Ok((h, s)) = attackers.get(a)
        {
            let enemy_kill = victim_faction.map(|(_, f)| f != s.faction).unwrap_or(true);
            if enemy_kill && let Some(slot) = humans.get_mut(h.id) {
                slot.kills += 1;
            }
        }
    }
}

/// Fill a player's HUD state from the authoritative world.
#[allow(clippy::type_complexity)]
pub fn private_state(
    slot: &HumanSlot,
    soldiers: &Query<(&Soldier, &Loadout, &Health, &Transform)>,
    vehicles: &Query<(&Vehicle, &Health, &Transform), Without<Soldier>>,
    squads: &Query<&Squad>,
    terrain: &Terrain,
) -> PrivateState {
    let mut st = PrivateState {
        respawn_timer: slot.respawn_timer.max(0.0),
        hurt: slot.hurt_count,
        hit: slot.hit_count,
        kills: slot.kills,
        deaths: slot.deaths,
        ..default()
    };
    if let Some(sq) = slot.squad.and_then(|s| squads.get(s).ok()) {
        st.squad_name.clone_from(&sq.name);
        st.order = sq.order.label().to_string();
        st.squad = sq
            .members
            .iter()
            .filter_map(|&m| soldiers.get(m).ok())
            .map(|(s, _, h, _)| (s.name.clone(), s.role as u8, !h.dead, 0))
            .collect();
    }
    let Some((s, l, h, _)) = slot.soldier.and_then(|e| soldiers.get(e).ok()) else {
        return st;
    };
    st.alive = !h.dead;
    st.hp = h.hp;
    st.stamina = s.stamina;
    st.stance = s.stance as u8;
    st.weapons = l.weapons.iter().map(|w| w.kind as u8).collect();
    st.active = l.active as u8;
    let w = l.current();
    st.ammo = w.ammo;
    st.mags = w.mags;
    st.reloading = w.reloading > 0.0;
    st.auto_mode = w.auto_mode;
    st.grenades = l.grenades;
    st.fak = s.fak;
    st.healing = s.healing > 0.0;
    st.shots = l.weapons.iter().map(|w| w.shots).sum();
    if let Some(ve) = s.vehicle
        && let Ok((v, vh, vtf)) = vehicles.get(ve)
    {
        st.in_vehicle = true;
        st.driver = v.driver == slot.soldier;
        st.vehicle_kind = v.kind as u8;
        st.vehicle_paint = v.paint as u8;
        st.vehicle_hp = vh.hp / vh.max;
        st.vehicle_speed = v.velocity.length();
        st.vehicle_alt = vtf.translation.y - terrain.height(vtf.translation.x, vtf.translation.z);
        let vw = &v.weapons[v.active];
        st.vehicle_weapon = vw.kind as u8;
        st.vehicle_ammo = vw.ammo;
        st.vehicle_mags = vw.mags;
        st.vehicle_reloading = vw.reloading > 0.0;
        st.shots = v.weapons.iter().map(|w| w.shots).sum();
    }
    st
}

#[allow(clippy::type_complexity)]
fn build_local_state(
    humans: Res<Humans>,
    mut local: ResMut<LocalState>,
    soldiers: Query<(&Soldier, &Loadout, &Health, &Transform)>,
    vehicles: Query<(&Vehicle, &Health, &Transform), Without<Soldier>>,
    squads: Query<&Squad>,
    terrain: Res<Terrain>,
) {
    if let Some(slot) = humans.get(local.id) {
        local.state = private_state(slot, &soldiers, &vehicles, &squads, &terrain);
        local.soldier = slot.soldier;
    }
}

/// Keep the `Player` marker on the locally controlled soldier.
fn mark_local_player(
    mut commands: Commands,
    local: Res<LocalState>,
    marked: Query<Entity, With<Player>>,
    exists: Query<(), With<Soldier>>,
) {
    let want = local.soldier.filter(|&e| exists.contains(e));
    for e in &marked {
        if Some(e) != want {
            commands.entity(e).remove::<Player>();
        }
    }
    if let Some(e) = want
        && !marked.contains(e)
    {
        commands.entity(e).insert(Player);
    }
}

fn local_feedback(
    local: Res<LocalState>,
    mut view: ResMut<LocalView>,
    mut fb: ResMut<PlayerFeedback>,
    time: Res<Time>,
) {
    let st = &local.state;
    let dt = time.delta_secs();
    fb.hurt = (fb.hurt - dt * 1.5).max(0.0);
    fb.hitmarker = (fb.hitmarker - dt * 4.0).max(0.0);
    if st.hurt != view.last_hurt {
        if st.hurt > view.last_hurt {
            fb.hurt = 1.0;
        }
        view.last_hurt = st.hurt;
    }
    if st.hit != view.last_hit {
        if st.hit > view.last_hit {
            fb.hitmarker = 1.0;
        }
        view.last_hit = st.hit;
    }
    // Recoil is a local view kick so it feels immediate even over the network.
    if st.shots != view.last_shots {
        let fired = st.shots.saturating_sub(view.last_shots).min(3);
        view.last_shots = st.shots;
        if st.alive && fired > 0 {
            let kind = if st.in_vehicle {
                WeaponKind::from_u8(st.vehicle_weapon)
            } else {
                WeaponKind::from_u8(st.weapons.get(st.active as usize).copied().unwrap_or(0))
            };
            let stance_k = match Stance::from_u8(st.stance) {
                Stance::Stand => 1.0,
                Stance::Crouch => 0.8,
                Stance::Prone => 0.55,
            };
            let ads_k = if view.ads > 0.5 { 0.7 } else { 1.0 };
            let r = kind.stats().recoil * stance_k * ads_k * fired as f32;
            if !st.in_vehicle {
                view.pitch += r * rand_range(0.8, 1.2);
                view.yaw += r * rand_range(-0.35, 0.35);
            }
        }
    }
    if !view.initialized && local.soldier.is_some() {
        view.initialized = true;
        view.yaw = -0.9;
    }
    view.dead = !st.alive;
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_camera(
    time: Res<Time>,
    local: Res<LocalState>,
    mut view: ResMut<LocalView>,
    terrain: Res<Terrain>,
    obstacles: Res<Obstacles>,
    soldiers: Query<(&Soldier, &Transform), Without<MainCamera>>,
    vehicles: Query<(&Vehicle, &Transform), (Without<MainCamera>, Without<Soldier>)>,
    mut cam: Single<(&mut Transform, &mut Projection), With<MainCamera>>,
    input: Res<LocalInput>,
) {
    let dt = time.delta_secs();
    let st = &local.state;
    let (cam_tf, proj) = &mut *cam;
    let Some((s, stf)) = local.soldier.and_then(|e| soldiers.get(e).ok()) else {
        // Not spawned yet: overview of the island.
        let t = time.elapsed_secs() * 0.03;
        cam_tf.translation = Vec3::new(t.cos() * 900.0, 420.0, t.sin() * 900.0);
        cam_tf.look_at(Vec3::new(0.0, 40.0, 0.0), Vec3::Y);
        return;
    };
    let want_ads = st.alive && !st.in_vehicle && !view.third_person;
    let ads_target = if want_ads && input.0.ads { 1.0 } else { 0.0 };
    view.ads += (ads_target - view.ads) * (dt * 12.0).min(1.0);
    let held = WeaponKind::from_u8(st.weapons.get(st.active as usize).copied().unwrap_or(0));
    let mut fov = 75f32;
    let rot = Quat::from_euler(
        EulerRot::YXZ,
        view.yaw + view.look_yaw,
        view.pitch + view.look_pitch,
        0.0,
    );
    if !st.alive {
        let t = time.elapsed_secs() * 0.2;
        let target = stf.translation + Vec3::Y * 0.5;
        let mut p = target + Vec3::new(t.cos() * 7.0, 5.0, t.sin() * 7.0);
        p.y = p.y.max(terrain.height(p.x, p.z) + 1.0);
        cam_tf.translation = p;
        cam_tf.look_at(target, Vec3::Y);
    } else if let Some((v, vtf)) = s.vehicle.and_then(|ve| vehicles.get(ve).ok()) {
        let spec = v.spec();
        if view.third_person || !st.driver {
            let target = v.center(vtf) + Vec3::Y * 1.5;
            let dir = rot * Vec3::NEG_Z;
            let mut p = target - dir * spec.cam_dist;
            if let Some(t) = terrain.raycast(target, p) {
                p = target.lerp(p, (t - 0.05).max(0.0));
            }
            p.y = p.y.max(terrain.height(p.x, p.z) + 0.8);
            cam_tf.translation = p;
            cam_tf.rotation = rot;
        } else {
            cam_tf.translation = vtf.translation + vtf.rotation * spec.eye;
            cam_tf.rotation = rot;
            if input.0.ads {
                fov = 22.0;
            }
        }
    } else if view.third_person {
        let eye = s.eye(stf.translation);
        let right = rot * Vec3::X;
        let target = eye + right * 0.55 + Vec3::Y * 0.15;
        let mut p = target - (rot * Vec3::NEG_Z) * 3.0;
        if let Some((t, _)) = obstacles.segment_hit(target, p, BULLET) {
            p = target.lerp(p, (t - 0.1).max(0.0));
        }
        if let Some(t) = terrain.raycast(target, p) {
            p = target.lerp(p, (t - 0.05).max(0.0));
        }
        cam_tf.translation = p;
        cam_tf.rotation = rot;
        if input.0.ads {
            fov = 55.0;
        }
    } else {
        cam_tf.translation = s.eye(stf.translation);
        cam_tf.rotation = rot;
        let ads_fov = if held == WeaponKind::Launcher { 30.0 } else { 42.0 };
        fov = 75.0 + (ads_fov - 75.0) * view.ads;
    }
    view.fov += (fov.to_radians() - view.fov) * (dt * 14.0).min(1.0);
    if let Projection::Perspective(p) = &mut **proj {
        p.fov = view.fov;
    }
    // What the crosshair is on: used for turret aiming and squad move orders.
    let origin = cam_tf.translation;
    let dir = cam_tf.rotation * Vec3::NEG_Z;
    let far = origin + dir * 2500.0;
    let mut t_hit = terrain.raycast(origin, far).unwrap_or(1.0);
    if let Some((t, _)) = obstacles.segment_hit(origin, origin.lerp(far, t_hit), BULLET) {
        t_hit *= t;
    }
    view.aim_point = origin.lerp(far, t_hit);
}

#[allow(clippy::type_complexity)]
fn update_view_model(
    time: Res<Time>,
    local: Res<LocalState>,
    view: Res<LocalView>,
    input: Res<LocalInput>,
    mut root: Single<(&mut Transform, &mut Visibility), With<ViewModel>>,
    mut parts: Query<(&ViewModelPart, &mut Visibility), Without<ViewModel>>,
    soldiers: Query<&Soldier>,
) {
    let st = &local.state;
    let held = WeaponKind::from_u8(st.weapons.get(st.active as usize).copied().unwrap_or(0));
    let show = st.alive && !st.in_vehicle && !view.third_person && !view.map_open && !st.healing;
    let (tf, vis) = &mut *root;
    **vis = if show {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    let gun = match held {
        WeaponKind::Launcher => ViewGun::Launcher,
        WeaponKind::Lmg | WeaponKind::Pkm => ViewGun::Mg,
        _ => ViewGun::Rifle,
    };
    for (p, mut v) in &mut parts {
        let want = if p.0 == gun {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *v != want {
            *v = want;
        }
    }
    let t = time.elapsed_secs();
    let sprinting = local
        .soldier
        .and_then(|e| soldiers.get(e).ok())
        .is_some_and(|s| s.sprinting);
    let moving = input.0.move_axis.length() > 0.1;
    let bob = if moving { (t * 9.0).sin() * 0.008 } else { 0.0 };
    let sway = Vec3::new((t * 0.9).sin() * 0.002, (t * 1.3).sin() * 0.0015, 0.0)
        * if view.ads > 0.5 { 0.6 } else { 1.0 };
    let hip = Vec3::new(0.13, -0.12, -0.3);
    let ads = Vec3::new(0.0, 0.0, -0.17);
    let mut pos = hip.lerp(ads, view.ads) + sway + Vec3::new(bob * 0.5, bob.abs(), 0.0);
    let mut rot = Quat::IDENTITY;
    if sprinting {
        pos += Vec3::new(0.05, -0.05, 0.05);
        rot = Quat::from_euler(EulerRot::YXZ, 0.6, -0.3, 0.2);
    }
    if st.reloading {
        pos += Vec3::new(0.0, -0.08, 0.0);
        rot = Quat::from_rotation_z(-0.5) * Quat::from_rotation_x(0.3);
    }
    let k = (time.delta_secs() * 14.0).min(1.0);
    tf.translation = tf.translation.lerp(pos, k);
    tf.rotation = tf.rotation.slerp(rot, k);
}
