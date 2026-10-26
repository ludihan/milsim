//! Operation Iron Ridge: an open-world military sandbox in the spirit of Arma, built on Bevy.
//!
//! Usage:
//!   milsim                         single player
//!   milsim host [port]             host a co-op session (default port 27015)
//!   milsim join <ip[:port]> [name] join a co-op session

#![allow(clippy::too_many_arguments, clippy::type_complexity)]

mod ai;
mod audio;
mod combat;
mod common;
mod devtools;
mod hud;
mod mission;
mod models;
mod net;
mod player;
mod soldier;
mod terrain;
mod vehicle;
mod world;

use std::net::{SocketAddr, ToSocketAddrs};

use bevy::{prelude::*, window::PresentMode};

use common::{Phase, SKY_COLOR};
use net::{DEFAULT_PORT, NetMode};

fn parse_args() -> Result<NetMode, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => Ok(NetMode::Single),
        Some("host") => {
            let port = match args.get(1) {
                Some(p) => p.parse().map_err(|_| format!("invalid port: {p}"))?,
                None => DEFAULT_PORT,
            };
            let mode = NetMode::host(port).map_err(|e| format!("cannot listen on port {port}: {e}"))?;
            println!("Hosting co-op on UDP port {port}");
            Ok(mode)
        }
        Some("join") => {
            let target = args.get(1).ok_or("usage: milsim join <ip[:port]> [name]")?;
            let with_port = if target.contains(':') {
                target.clone()
            } else {
                format!("{target}:{DEFAULT_PORT}")
            };
            let addr: SocketAddr = with_port
                .to_socket_addrs()
                .map_err(|e| format!("cannot resolve {with_port}: {e}"))?
                .next()
                .ok_or_else(|| format!("cannot resolve {with_port}"))?;
            let name = args
                .get(2)
                .cloned()
                .or_else(|| std::env::var("USER").ok())
                .unwrap_or_else(|| "Soldier".into());
            println!("Joining {addr} as {name}");
            NetMode::join(addr, name).map_err(|e| format!("cannot open socket: {e}"))
        }
        Some(other) => Err(format!(
            "unknown command `{other}`\nusage: milsim [host [port] | join <ip[:port]> [name]]"
        )),
    }
}

fn main() {
    let mode = match parse_args() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let is_client = mode.is_client();

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Operation Iron Ridge".into(),
            resolution: (1600, 900).into(),
            present_mode: PresentMode::AutoVsync,
            ..default()
        }),
        ..default()
    }))
    .insert_resource(ClearColor(SKY_COLOR))
    .insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.75, 0.82, 1.0),
        brightness: 900.0,
        ..default()
    })
    .insert_resource(mode)
    .insert_resource(terrain::Terrain::generate())
    .configure_sets(
        Update,
        (
            Phase::Input,
            Phase::Ai,
            Phase::Movement,
            Phase::Combat,
            Phase::Damage,
            Phase::Camera,
            Phase::Hud,
        )
            .chain(),
    )
    .add_plugins((
        world::WorldPlugin,
        combat::CombatPlugin,
        soldier::SoldierPlugin,
        vehicle::VehiclePlugin,
        ai::AiPlugin,
        player::PlayerPlugin,
        audio::AudioPlugin,
        net::NetPlugin,
        mission::MissionPlugin,
        hud::HudPlugin,
        devtools::DevToolsPlugin,
    ));
    if !is_client {
        // The host is player 0.
        app.add_systems(Startup, |mut humans: ResMut<player::Humans>| {
            let name = std::env::var("USER").unwrap_or_else(|_| "Player".into());
            humans.add(0, name);
        });
    }
    app.run();
}
