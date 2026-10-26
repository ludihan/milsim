//! Procedurally synthesized sound effects with distance falloff, panning and speed-of-sound delay.

use std::collections::HashMap;
use std::f32::consts::TAU;

use bevy::{
    audio::{AudioSink, AudioSinkPlayback, PlaybackMode, SpatialScale, Volume},
    prelude::*,
};

use crate::common::Phase;
use crate::player::{LocalState, MainCamera};
use crate::vehicle::{Vehicle, VehicleKind};

const SAMPLE_RATE: u32 = 22050;
const SPEED_OF_SOUND: f32 = 343.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[repr(u8)]
pub enum SoundKind {
    RifleShot,
    AkShot,
    MgShot,
    HmgShot,
    CannonShot,
    LauncherShot,
    Explosion,
    Crack,
    Impact,
    Reload,
    DryFire,
    Throw,
    Door,
}

impl SoundKind {
    const ALL: [SoundKind; 13] = [
        SoundKind::RifleShot,
        SoundKind::AkShot,
        SoundKind::MgShot,
        SoundKind::HmgShot,
        SoundKind::CannonShot,
        SoundKind::LauncherShot,
        SoundKind::Explosion,
        SoundKind::Crack,
        SoundKind::Impact,
        SoundKind::Reload,
        SoundKind::DryFire,
        SoundKind::Throw,
        SoundKind::Door,
    ];

    pub fn from_u8(v: u8) -> Self {
        Self::ALL.get(v as usize).copied().unwrap_or(SoundKind::Impact)
    }

    /// Loud sounds that carry across the map get a muffled distant variant.
    fn carries(self) -> bool {
        matches!(
            self,
            SoundKind::RifleShot
                | SoundKind::AkShot
                | SoundKind::MgShot
                | SoundKind::HmgShot
                | SoundKind::CannonShot
                | SoundKind::LauncherShot
                | SoundKind::Explosion
        )
    }

    /// Distance (m) at which the sound is at half volume.
    fn reach(self) -> f32 {
        match self {
            SoundKind::Explosion | SoundKind::CannonShot => 120.0,
            SoundKind::HmgShot => 80.0,
            SoundKind::RifleShot | SoundKind::AkShot | SoundKind::MgShot | SoundKind::LauncherShot => 45.0,
            _ => 6.0,
        }
    }
}

#[derive(Message, Clone, Copy)]
pub struct PlaySound {
    pub kind: SoundKind,
    pub pos: Vec3,
    pub volume: f32,
    /// Only heard on this machine (not replicated).
    pub local_only: bool,
}

impl PlaySound {
    pub fn new(kind: SoundKind, pos: Vec3) -> Self {
        Self {
            kind,
            pos,
            volume: 1.0,
            local_only: false,
        }
    }

    pub fn local(mut self) -> Self {
        self.local_only = true;
        self
    }

    pub fn volume(mut self, v: f32) -> Self {
        self.volume = v;
        self
    }
}

#[derive(Resource)]
struct Sfx {
    near: HashMap<SoundKind, Handle<AudioSource>>,
    far: HashMap<SoundKind, Handle<AudioSource>>,
}

#[derive(Component)]
struct Delayed {
    timer: f32,
    handle: Handle<AudioSource>,
    volume: f32,
    pos: Vec3,
}

#[derive(Component, Clone, Copy, PartialEq)]
enum Loop {
    Engine,
    Rotor,
}

#[derive(Component)]
struct OneShot;

pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<PlaySound>()
            .add_systems(Startup, setup_audio)
            .add_systems(
                Update,
                (play_sounds, play_delayed, vehicle_loops).in_set(Phase::Hud),
            );
    }
}

// ---------------------------------------------------------------------------
// Synthesis
// ---------------------------------------------------------------------------

fn encode_wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32000.0) as i16).to_le_bytes());
    }
    out
}

fn normalize(mut s: Vec<f32>, peak: f32) -> Vec<f32> {
    let max = s.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    for v in s.iter_mut() {
        *v *= peak / max;
    }
    s
}

fn len(secs: f32) -> usize {
    (secs * SAMPLE_RATE as f32) as usize
}

fn noise(rng: &mut fastrand::Rng) -> f32 {
    rng.f32() * 2.0 - 1.0
}

/// Gunshot: sharp crack + body thump + rolling tail.
fn gunshot(rng: &mut fastrand::Rng, body_hz: f32, crack: f32, decay: f32, tail: f32) -> Vec<f32> {
    let n = len(0.25 + tail);
    let mut out = Vec::with_capacity(n);
    let (mut lp, mut lp2) = (0.0f32, 0.0f32);
    for i in 0..n {
        let t = i as f32 / SAMPLE_RATE as f32;
        let x = noise(rng);
        lp += (x - lp) * 0.25;
        lp2 += (x - lp2) * 0.04;
        let s = x * crack * (-t * decay * 4.0).exp()
            + lp * 1.3 * (-t * decay).exp()
            + (TAU * body_hz * t).sin() * 0.9 * (-t * 28.0).exp()
            + lp2 * 1.5 * (-t * 3.0 / tail).exp();
        out.push(s);
    }
    normalize(out, 0.95)
}

/// The same sound heard from afar: no crack, muffled, with echoes.
fn distant(src: &[f32]) -> Vec<f32> {
    let extra = len(0.9);
    let mut out = vec![0.0f32; src.len() + extra];
    let mut lp = 0.0f32;
    let mut lp2 = 0.0f32;
    for (i, &s) in src.iter().enumerate() {
        lp += (s - lp) * 0.06;
        lp2 += (lp - lp2) * 0.15;
        out[i] += lp2 * 2.5;
    }
    for (delay, gain) in [(0.11, 0.45), (0.27, 0.3), (0.52, 0.18)] {
        let d = len(delay);
        for i in (0..out.len() - d).rev() {
            out[i + d] += out[i] * gain;
        }
    }
    normalize(out, 0.9)
}

fn explosion(rng: &mut fastrand::Rng) -> Vec<f32> {
    let n = len(2.8);
    let mut out = Vec::with_capacity(n);
    let (mut brown, mut lp) = (0.0f32, 0.0f32);
    for i in 0..n {
        let t = i as f32 / SAMPLE_RATE as f32;
        let x = noise(rng);
        brown = (brown + x * 0.08) * 0.995;
        lp += (x - lp) * 0.12;
        let thump = (TAU * (55.0 - t * 15.0) * t).sin() * (-t * 6.0).exp();
        let crackle = if rng.f32() < 0.004 * (-t * 2.0).exp() { noise(rng) * 2.0 } else { 0.0 };
        out.push(brown * 3.0 * (-t * 1.4).exp() + lp * 1.5 * (-t * 8.0).exp() + thump * 1.5 + crackle);
    }
    normalize(out, 0.98)
}

fn launcher(rng: &mut fastrand::Rng) -> Vec<f32> {
    let n = len(1.2);
    let mut out = Vec::with_capacity(n);
    let (mut lp, mut bp) = (0.0f32, 0.0f32);
    for i in 0..n {
        let t = i as f32 / SAMPLE_RATE as f32;
        let x = noise(rng);
        lp += (x - lp) * 0.3;
        bp += (lp - bp) * 0.08;
        let pop = x * (-t * 40.0).exp() + (TAU * 80.0 * t).sin() * (-t * 20.0).exp();
        let whoosh = (lp - bp) * (t * 20.0).min(1.0) * (-t * 2.2).exp();
        out.push(pop + whoosh * 1.4);
    }
    normalize(out, 0.95)
}

fn crack(rng: &mut fastrand::Rng) -> Vec<f32> {
    let n = len(0.09);
    let mut lp = 0.0f32;
    let out = (0..n)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let x = noise(rng);
            lp += (x - lp) * 0.2;
            (x - lp) * (-t * 90.0).exp()
        })
        .collect();
    normalize(out, 0.8)
}

fn thud(rng: &mut fastrand::Rng, secs: f32, hz: f32, decay: f32) -> Vec<f32> {
    let n = len(secs);
    let mut lp = 0.0f32;
    let out = (0..n)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let x = noise(rng);
            lp += (x - lp) * 0.1;
            (lp * 2.0 + (TAU * hz * t).sin()) * (-t * decay).exp()
        })
        .collect();
    normalize(out, 0.8)
}

fn clicks(rng: &mut fastrand::Rng, at: &[f32], total: f32) -> Vec<f32> {
    let n = len(total);
    let mut out = vec![0.0f32; n];
    for &start in at {
        let s0 = len(start);
        for i in 0..len(0.05) {
            let t = i as f32 / SAMPLE_RATE as f32;
            if s0 + i >= n {
                break;
            }
            let v = ((TAU * 2600.0 * t).sin() * 0.6 + (TAU * 3900.0 * t).sin() * 0.4 + noise(rng) * 0.5)
                * (-t * 90.0).exp();
            out[s0 + i] += v;
        }
    }
    normalize(out, 0.6)
}

/// Seamless 1-second loop: low sawtooth rumble.
fn engine_loop(rng: &mut fastrand::Rng) -> Vec<f32> {
    let n = SAMPLE_RATE as usize;
    let mut lp = 0.0f32;
    let out = (0..n)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let mut s = 0.0;
            for h in 1..8 {
                s += (TAU * 40.0 * h as f32 * t).sin() / h as f32;
            }
            lp += (noise(rng) - lp) * 0.05;
            s * (1.0 + 0.3 * (TAU * 10.0 * t).sin()) + lp * 0.8
        })
        .collect();
    normalize(out, 0.7)
}

/// Seamless 1-second loop: blade slap at 6 Hz plus turbine whine.
fn rotor_loop(rng: &mut fastrand::Rng) -> Vec<f32> {
    let n = SAMPLE_RATE as usize;
    let mut lp = 0.0f32;
    let out = (0..n)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let phase = (t * 6.0).fract();
            lp += (noise(rng) - lp) * 0.15;
            lp * 2.5 * (-phase * 9.0).exp()
                + (TAU * 1200.0 * t).sin() * 0.08
                + (TAU * 60.0 * t).sin() * 0.3
        })
        .collect();
    normalize(out, 0.7)
}

fn setup_audio(mut commands: Commands, mut sources: ResMut<Assets<AudioSource>>) {
    let mut rng = fastrand::Rng::with_seed(7);
    let mut add = |s: Vec<f32>| {
        sources.add(AudioSource {
            bytes: encode_wav(&s).into(),
        })
    };
    let mut near = HashMap::new();
    let mut far = HashMap::new();
    let defs: Vec<(SoundKind, Vec<f32>)> = vec![
        (SoundKind::RifleShot, gunshot(&mut rng, 140.0, 1.0, 35.0, 0.35)),
        (SoundKind::AkShot, gunshot(&mut rng, 110.0, 0.9, 28.0, 0.45)),
        (SoundKind::MgShot, gunshot(&mut rng, 120.0, 0.9, 30.0, 0.4)),
        (SoundKind::HmgShot, gunshot(&mut rng, 70.0, 0.8, 18.0, 0.7)),
        (SoundKind::CannonShot, gunshot(&mut rng, 50.0, 0.7, 10.0, 1.3)),
        (SoundKind::LauncherShot, launcher(&mut rng)),
        (SoundKind::Explosion, explosion(&mut rng)),
        (SoundKind::Crack, crack(&mut rng)),
        (SoundKind::Impact, thud(&mut rng, 0.1, 180.0, 45.0)),
        (SoundKind::Reload, clicks(&mut rng, &[0.0, 0.45, 1.3], 1.5)),
        (SoundKind::DryFire, clicks(&mut rng, &[0.0], 0.1)),
        (SoundKind::Throw, clicks(&mut rng, &[0.0, 0.12], 0.3)),
        (SoundKind::Door, thud(&mut rng, 0.25, 90.0, 18.0)),
    ];
    for (kind, samples) in defs {
        if kind.carries() {
            far.insert(kind, add(distant(&samples)));
        }
        near.insert(kind, add(samples));
    }
    let engine = add(engine_loop(&mut rng));
    let rotor = add(rotor_loop(&mut rng));
    commands.insert_resource(Sfx { near, far });
    for (handle, kind) in [(engine, Loop::Engine), (rotor, Loop::Rotor)] {
        commands.spawn((
            AudioPlayer(handle),
            PlaybackSettings {
                mode: PlaybackMode::Loop,
                volume: Volume::Linear(0.0),
                ..default()
            },
            kind,
        ));
    }
}

/// Emitters sit right next to the listener in the true direction: the spatial sink only pans,
/// distance falloff is ours.
fn emit(commands: &mut Commands, handle: Handle<AudioSource>, volume: f32, pos: Vec3, listener: &Transform) {
    let dir = (pos - listener.translation).normalize_or(listener.forward().as_vec3());
    commands.spawn((
        AudioPlayer(handle),
        PlaybackSettings {
            mode: PlaybackMode::Despawn,
            volume: Volume::Linear(volume),
            spatial: true,
            spatial_scale: Some(SpatialScale::new(0.5)),
            ..default()
        },
        Transform::from_translation(listener.translation + dir * 0.8),
        OneShot,
    ));
}

fn play_sounds(
    mut commands: Commands,
    mut events: MessageReader<PlaySound>,
    sfx: Res<Sfx>,
    listener: Single<&Transform, With<MainCamera>>,
    active: Query<(), With<OneShot>>,
) {
    let mut budget = 40usize.saturating_sub(active.iter().count());
    for ev in events.read() {
        let d = ev.pos.distance(listener.translation);
        let reach = ev.kind.reach();
        let mut volume = ev.volume / (1.0 + (d / reach).powf(1.6));
        let use_far = ev.kind.carries() && d > 150.0;
        if use_far {
            volume *= 2.5;
        }
        volume = volume.min(1.5);
        if volume < 0.01 {
            continue;
        }
        let map = if use_far { &sfx.far } else { &sfx.near };
        let Some(handle) = map.get(&ev.kind).cloned() else {
            continue;
        };
        let delay = if d > 40.0 { d / SPEED_OF_SOUND } else { 0.0 };
        if delay > 0.0 {
            commands.spawn(Delayed {
                timer: delay,
                handle,
                volume,
                pos: ev.pos,
            });
        } else if budget > 0 || volume > 0.3 {
            budget = budget.saturating_sub(1);
            emit(&mut commands, handle, volume, ev.pos, &listener);
        }
    }
}

fn play_delayed(
    mut commands: Commands,
    time: Res<Time>,
    mut q: Query<(Entity, &mut Delayed)>,
    listener: Single<&Transform, With<MainCamera>>,
) {
    for (e, mut d) in &mut q {
        d.timer -= time.delta_secs();
        if d.timer <= 0.0 {
            emit(&mut commands, d.handle.clone(), d.volume, d.pos, &listener);
            commands.entity(e).despawn();
        }
    }
}

fn vehicle_loops(
    local: Res<LocalState>,
    listener: Single<&Transform, With<MainCamera>>,
    vehicles: Query<(&Vehicle, &Transform)>,
    mut sinks: Query<(&Loop, &mut AudioSink)>,
) {
    // Loudest nearby engine/rotor of each type drives the shared loop.
    let mut engine = (0.0f32, 1.0f32);
    let mut rotor = (0.0f32, 1.0f32);
    for (v, tf) in &vehicles {
        if v.destroyed {
            continue;
        }
        let d = tf.translation.distance(listener.translation);
        let inside = local.state.in_vehicle && d < 6.0;
        let near = if inside { 0.55 } else { 1.0 / (1.0 + (d / 30.0).powf(1.5)) };
        if v.kind == VehicleKind::Heli {
            let vol = near * v.rotor * 0.9;
            if vol > rotor.0 {
                rotor = (vol, 0.6 + v.rotor * 0.4);
            }
        } else if v.driver.is_some() || v.crew_faction.is_some() {
            let load = (v.speed.abs() / v.spec().max_speed).min(1.0);
            let vol = near * (0.35 + load * 0.4);
            if vol > engine.0 {
                engine = (vol, 0.7 + load * 0.8);
            }
        }
    }
    for (kind, mut sink) in &mut sinks {
        let (vol, speed) = match kind {
            Loop::Engine => engine,
            Loop::Rotor => rotor,
        };
        sink.set_volume(Volume::Linear(vol));
        sink.set_speed(speed);
    }
}
