//! Developer tour: `MILSIM_TOUR=<dir> cargo dev` plays a scripted sequence (first person, third
//! person, map, aiming, driving, approaching a town), saves screenshots into `<dir>` and exits.

use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, save_to_disk},
};

use crate::common::*;
use crate::mission::BaseVehicle;
use crate::player::{E_INTERACT, E_RESPAWN, LocalInput, LocalView, read_local_input};
use crate::terrain::{LOCATIONS, Terrain};
use crate::vehicle::{Vehicle, VehicleKind};

pub struct DevToolsPlugin;

impl Plugin for DevToolsPlugin {
    fn build(&self, app: &mut App) {
        let Ok(dir) = std::env::var("MILSIM_TOUR") else {
            return;
        };
        std::fs::create_dir_all(&dir).ok();
        app.insert_resource(Tour {
            dir,
            time: 0.0,
            step: 0,
            frames: 0,
        })
        .add_systems(
            Update,
            tour.after(read_local_input)
                .before(crate::player::copy_local_input)
                .in_set(Phase::Input),
        );
    }
}

#[derive(Resource)]
struct Tour {
    dir: String,
    time: f32,
    step: usize,
    frames: u32,
}

#[derive(Clone, Copy)]
enum Act {
    Shot(&'static str),
    ThirdPerson(bool),
    Map(bool),
    Ads(bool),
    Board(VehicleKind),
    Drive(bool),
    Climb(bool),
    Turn(f32),
    Respawn,
    Interact,
    GoToTown(usize, f32),
    Exit,
}

const SCRIPT: &[(f32, Act)] = &[
    (8.0, Act::Shot("01_first_person")),
    (8.5, Act::ThirdPerson(true)),
    (9.5, Act::Shot("02_third_person")),
    (9.6, Act::Turn(std::f32::consts::PI)),
    (9.9, Act::Shot("02b_squad")),
    (9.95, Act::Turn(std::f32::consts::PI)),
    (10.0, Act::ThirdPerson(false)),
    (10.1, Act::Map(true)),
    (11.0, Act::Shot("03_map")),
    (11.5, Act::Map(false)),
    (11.6, Act::Ads(true)),
    (12.5, Act::Shot("04_aiming")),
    (13.0, Act::Ads(false)),
    (13.1, Act::Board(VehicleKind::Jeep)),
    (13.5, Act::Interact),
    (14.0, Act::ThirdPerson(true)),
    (15.0, Act::Shot("05_jeep")),
    (15.1, Act::Drive(true)),
    (19.0, Act::Shot("06_driving")),
    (19.1, Act::Drive(false)),
    (20.0, Act::Interact),
    (21.0, Act::ThirdPerson(false)),
    (21.1, Act::GoToTown(1, 320.0)),
    (27.0, Act::Shot("07_town_view")),
    (27.1, Act::GoToTown(1, 140.0)),
    (27.2, Act::ThirdPerson(true)),
    (36.0, Act::Shot("08_town_close")),
    // The town garrison usually wins; redeploy if needed before flying.
    (40.0, Act::Respawn),
    (42.0, Act::Respawn),
    (44.0, Act::Board(VehicleKind::Heli)),
    (44.5, Act::Interact),
    (45.0, Act::ThirdPerson(true)),
    (50.0, Act::Climb(true)),
    (55.0, Act::Shot("09_heli_takeoff")),
    (55.1, Act::Climb(false)),
    (55.2, Act::Drive(true)),
    (60.0, Act::Shot("10_heli_flight")),
    (60.5, Act::Exit),
];

/// Clients can't teleport (the host is authoritative), so their tour only looks around.
const CLIENT_SCRIPT: &[(f32, Act)] = &[
    (12.0, Act::Shot("c01_client_first_person")),
    (12.5, Act::ThirdPerson(true)),
    (14.0, Act::Shot("c02_client_third_person")),
    (14.5, Act::Exit),
];

#[allow(clippy::too_many_arguments)]
fn tour(
    mut commands: Commands,
    time: Res<Time>,
    mut tour: ResMut<Tour>,
    mut view: ResMut<LocalView>,
    mut input: ResMut<LocalInput>,
    terrain: Res<Terrain>,
    mut players: Query<&mut Transform, With<Player>>,
    vehicles: Query<(&Vehicle, &Transform), (With<BaseVehicle>, Without<Player>)>,
    mut exit: MessageWriter<AppExit>,
    mut driving: Local<bool>,
    mut ads: Local<bool>,
    mut climbing: Local<bool>,
    net: Res<crate::net::NetMode>,
) {
    let script = if net.is_client() { CLIENT_SCRIPT } else { SCRIPT };
    tour.time += time.delta_secs();
    tour.frames += 1;
    input.0.ads = *ads;
    if *driving {
        input.0.move_axis = Vec2::new(0.15, 1.0);
    }
    if *climbing {
        input.0.up = true;
    }
    while let Some(&(at, act)) = script.get(tour.step) {
        if tour.time < at {
            break;
        }
        tour.step += 1;
        match act {
            Act::Shot(name) => {
                let path = format!("{}/{name}.png", tour.dir);
                info!("tour: saving {path}");
                commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
            }
            Act::ThirdPerson(on) => view.third_person = on,
            Act::Map(on) => view.map_open = on,
            Act::Ads(on) => *ads = on,
            Act::Drive(on) => *driving = on,
            Act::Climb(on) => *climbing = on,
            Act::Turn(a) => view.yaw += a,
            Act::Interact => input.0.bump(E_INTERACT),
            Act::Respawn => input.0.bump(E_RESPAWN),
            Act::Board(kind) => {
                if let Some((_, vtf)) = vehicles.iter().find(|(v, _)| v.kind == kind)
                    && let Ok(mut tf) = players.single_mut()
                {
                    let side = vtf.rotation * Vec3::X * 2.0;
                    tf.translation = vtf.translation + side;
                    view.yaw = vtf.rotation.to_euler(EulerRot::YXZ).0;
                    view.pitch = -0.15;
                }
            }
            Act::GoToTown(loc, dist) => {
                let target = LOCATIONS[loc].pos;
                let from = target + Vec2::new(-0.7, 0.7).normalize() * dist;
                if let Ok(mut tf) = players.single_mut() {
                    tf.translation = Vec3::new(from.x, terrain.height(from.x, from.y), from.y);
                    let d = target - from;
                    view.yaw = f32::atan2(-d.x, -d.y);
                    view.pitch = -0.05;
                }
            }
            Act::Exit => {
                let fps = tour.frames as f32 / tour.time.max(0.001);
                info!("tour: done, average {fps:.1} fps over {:.0} s", tour.time);
                exit.write(AppExit::Success);
            }
        }
    }
}
