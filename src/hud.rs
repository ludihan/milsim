//! Heads-up display and the tactical map.

use bevy::prelude::*;

use crate::combat::WeaponKind;
use crate::common::*;
use crate::mission::Sectors;
use crate::net::{NetIndex, NetMode};
use crate::player::{LocalState, LocalView, MainCamera};
use crate::soldier::{Role, Soldier, Stance};
use crate::terrain::{BASE, LOCATIONS, MAP_HALF, Terrain, map_image};
use crate::vehicle::{Vehicle, VehicleKind};
use crate::world::MapFeatures;

#[derive(Component)]
struct CompassText;
#[derive(Component)]
struct InfoText;
#[derive(Component)]
struct WeaponText;
#[derive(Component)]
struct StatusText;
#[derive(Component)]
struct HealthBar;
#[derive(Component)]
struct StaminaBar;
#[derive(Component)]
struct SquadText;
#[derive(Component)]
struct PromptText;
#[derive(Component)]
struct BigText;
#[derive(Component)]
struct Crosshair;
#[derive(Component)]
struct GunMarker;
#[derive(Component)]
struct HitMarker;
#[derive(Component)]
struct Vignette;
#[derive(Component)]
struct FeedLine(usize);
#[derive(Component)]
struct NameTag(usize);
#[derive(Component)]
struct MapRoot;
#[derive(Component)]
struct MapPanel;
#[derive(Component)]
struct MapMarker;
#[derive(Component)]
struct SectorLabel(usize);

const FEED_LINES: usize = 7;
const NAME_TAGS: usize = 8;
const MAP_MARKERS: usize = 160;
const MAP_PX: f32 = 860.0;

#[derive(Resource, Default)]
struct Feed {
    lines: Vec<(String, Color, f32)>,
}

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Feed>()
            .add_message::<Radio>()
            .add_systems(Startup, setup_hud)
            .add_systems(
                Update,
                (
                    update_feed,
                    update_texts,
                    update_crosshair,
                    update_name_tags,
                    update_map,
                )
                    .chain()
                    .in_set(Phase::Hud),
            );
    }
}

fn font(size: f32) -> TextFont {
    TextFont {
        font_size: FontSize::Px(size),
        ..default()
    }
}

fn abs(left: Option<f32>, top: Option<f32>, right: Option<f32>, bottom: Option<f32>) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: left.map_or(Val::Auto, Val::Px),
        top: top.map_or(Val::Auto, Val::Px),
        right: right.map_or(Val::Auto, Val::Px),
        bottom: bottom.map_or(Val::Auto, Val::Px),
        ..default()
    }
}

fn setup_hud(
    mut commands: Commands,
    terrain: Res<Terrain>,
    features: Res<MapFeatures>,
    mut images: ResMut<Assets<Image>>,
) {
    let white = Color::srgba(0.95, 0.95, 0.9, 0.95);
    let dim = Color::srgba(0.85, 0.88, 0.8, 0.85);
    let shadow = TextShadow::default();

    commands.spawn((
        Vignette,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.6, 0.0, 0.0, 0.0)),
    ));

    commands.spawn((
        CompassText,
        Text::new(""),
        font(17.0),
        TextColor(white),
        TextLayout::default().with_justify(Justify::Center),
        shadow,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(10.0),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
    ));
    commands.spawn((InfoText, Text::new(""), font(15.0), TextColor(dim), shadow, abs(Some(16.0), Some(12.0), None, None)));
    commands.spawn((
        WeaponText,
        Text::new(""),
        font(17.0),
        TextColor(white),
        TextLayout::default().with_justify(Justify::Right),
        shadow,
        abs(None, None, Some(20.0), Some(18.0)),
    ));

    // Bottom-left: status bars.
    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            left: Val::Px(16.0),
            bottom: Val::Px(16.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(4.0),
            ..default()
        })
        .with_children(|p| {
            p.spawn((StatusText, Text::new(""), font(15.0), TextColor(white), shadow));
            for (marker, color) in [(0, Color::srgb(0.85, 0.2, 0.15)), (1, Color::srgb(0.85, 0.8, 0.35))] {
                p.spawn((
                    Node {
                        width: Val::Px(220.0),
                        height: Val::Px(7.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
                ))
                .with_children(|b| {
                    let mut e = b.spawn((
                        Node {
                            width: Val::Percent(100.0),
                            height: Val::Percent(100.0),
                            ..default()
                        },
                        BackgroundColor(color),
                    ));
                    if marker == 0 {
                        e.insert(HealthBar);
                    } else {
                        e.insert(StaminaBar);
                    }
                });
            }
        });

    // Radio feed, above the status block.
    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            left: Val::Px(16.0),
            bottom: Val::Px(92.0),
            flex_direction: FlexDirection::Column,
            ..default()
        })
        .with_children(|p| {
            for i in 0..FEED_LINES {
                p.spawn((FeedLine(i), Text::new(""), font(15.0), TextColor(white), shadow));
            }
        });

    commands.spawn((
        SquadText,
        Text::new(""),
        font(14.0),
        TextColor(dim),
        TextLayout::default().with_justify(Justify::Center),
        shadow,
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(14.0),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
    ));
    commands.spawn((
        PromptText,
        Text::new(""),
        font(18.0),
        TextColor(Color::srgb(1.0, 0.92, 0.6)),
        TextLayout::default().with_justify(Justify::Center),
        shadow,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Percent(60.0),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
    ));
    commands.spawn((
        BigText,
        Text::new(""),
        font(26.0),
        TextColor(Color::WHITE),
        TextLayout::default().with_justify(Justify::Center),
        shadow,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Percent(30.0),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
    ));

    // Crosshair, gun marker and hit marker: small centred texts.
    let centered = |w: f32| Node {
        position_type: PositionType::Absolute,
        left: Val::Percent(50.0),
        top: Val::Percent(50.0),
        width: Val::Px(w),
        height: Val::Px(w),
        margin: UiRect {
            left: Val::Px(-w / 2.0),
            top: Val::Px(-w / 2.0),
            ..default()
        },
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    };
    commands
        .spawn((Crosshair, centered(40.0)))
        .with_child((Text::new("+"), font(24.0), TextColor(Color::srgba(1.0, 1.0, 1.0, 0.75))));
    commands
        .spawn((GunMarker, centered(40.0), Visibility::Hidden))
        .with_child((Text::new("( )"), font(18.0), TextColor(Color::srgba(0.4, 1.0, 0.4, 0.9))));
    commands
        .spawn((HitMarker, centered(40.0)))
        .with_child((Text::new("x"), font(30.0), TextColor(Color::srgba(1.0, 1.0, 1.0, 0.0))));

    for i in 0..NAME_TAGS {
        commands.spawn((
            NameTag(i),
            Text::new(""),
            font(13.0),
            TextColor(Color::srgba(0.5, 1.0, 0.5, 0.9)),
            shadow,
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            Visibility::Hidden,
        ));
    }

    // Map overlay.
    let image = images.add(map_image(&terrain, &features.buildings, &features.trees));
    commands
        .spawn((
            MapRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            Visibility::Hidden,
        ))
        .with_children(|p| {
            p.spawn((
                MapPanel,
                ImageNode::new(image),
                Node {
                    width: Val::Px(MAP_PX),
                    height: Val::Px(MAP_PX),
                    ..default()
                },
            ))
            .with_children(|m| {
                for _ in 0..MAP_MARKERS {
                    m.spawn((
                        MapMarker,
                        Node {
                            position_type: PositionType::Absolute,
                            width: Val::Px(6.0),
                            height: Val::Px(6.0),
                            ..default()
                        },
                        BackgroundColor(Color::WHITE),
                        Visibility::Hidden,
                    ));
                }
                for (i, loc) in LOCATIONS.iter().enumerate() {
                    let x = (loc.pos.x + MAP_HALF) / (2.0 * MAP_HALF) * MAP_PX;
                    let y = (loc.pos.y + MAP_HALF) / (2.0 * MAP_HALF) * MAP_PX;
                    m.spawn((
                        SectorLabel(i),
                        Text::new(loc.name),
                        font(15.0),
                        TextColor(Color::BLACK),
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(x + 8.0),
                            top: Val::Px(y - 9.0),
                            ..default()
                        },
                    ));
                }
                m.spawn((
                    Text::new(
                        "WASD move  Shift sprint  C crouch  Z prone  Alt freelook  V 3rd person\n\
                         LMB fire  RMB aim  R reload  B fire mode  1/2/X weapons  G grenade  H first aid\n\
                         PgUp/PgDn zeroing  F enter/exit vehicle  F1 form on me  F2 hold  F3 move to crosshair\n\
                         Vehicles: WASD drive, Space brake, mouse aims the gun, X switch weapon\n\
                         Helicopter: Shift/Ctrl collective, W/S pitch, A/D yaw, Q/E roll",
                    ),
                    font(13.0),
                    TextColor(Color::srgba(0.05, 0.05, 0.1, 0.9)),
                    BackgroundColor(Color::srgba(1.0, 1.0, 0.95, 0.7)),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(8.0),
                        bottom: Val::Px(8.0),
                        padding: UiRect::all(Val::Px(6.0)),
                        ..default()
                    },
                ));
            });
        });
}

fn update_feed(
    time: Res<Time>,
    mut events: MessageReader<Radio>,
    mut feed: ResMut<Feed>,
    mut lines: Query<(&FeedLine, &mut Text, &mut TextColor)>,
) {
    let now = time.elapsed_secs();
    for ev in events.read() {
        feed.lines.push((ev.text.clone(), ev.color, now));
    }
    feed.lines.retain(|(_, _, t)| now - t < 14.0);
    if feed.lines.len() > FEED_LINES {
        let extra = feed.lines.len() - FEED_LINES;
        feed.lines.drain(..extra);
    }
    for (line, mut text, mut color) in &mut lines {
        match feed.lines.get(line.0) {
            Some((s, c, t)) => {
                text.0.clone_from(s);
                let fade = (14.0 - (now - t)).clamp(0.0, 2.0) / 2.0;
                color.0 = c.with_alpha(fade);
            }
            None => text.0.clear(),
        }
    }
}

fn compass_tape(heading_deg: f32) -> String {
    let mut s = String::new();
    for i in -20..=20 {
        let deg = (heading_deg + i as f32 * 3.0).rem_euclid(360.0);
        let d = (deg / 3.0).round() as i32 * 3;
        let label = match d.rem_euclid(360) {
            0 => "N",
            45 => "NE",
            90 => "E",
            135 => "SE",
            180 => "S",
            225 => "SW",
            270 => "W",
            315 => "NW",
            x if x % 15 == 0 => "|",
            _ => "·",
        };
        if label.len() == 2 {
            s.push_str(label);
        } else {
            s.push_str(label);
            s.push(' ');
        }
    }
    s
}

/// Compass heading of a yaw (0 = north = -Z), in degrees clockwise.
fn heading(yaw: f32) -> f32 {
    (-yaw.to_degrees()).rem_euclid(360.0)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_texts(
    local: Res<LocalState>,
    view: Res<LocalView>,
    net: Res<NetMode>,
    sectors: Res<Sectors>,
    fb: Res<PlayerFeedback>,
    soldiers: Query<(&Soldier, &Transform)>,
    vehicles: Query<(&Vehicle, &Transform)>,
    mut texts: ParamSet<(
        Single<&mut Text, With<CompassText>>,
        Single<&mut Text, With<InfoText>>,
        Single<&mut Text, With<WeaponText>>,
        Single<&mut Text, With<StatusText>>,
        Single<&mut Text, With<SquadText>>,
        Single<&mut Text, With<PromptText>>,
        Single<&mut Text, With<BigText>>,
    )>,
    mut bars: ParamSet<(
        Single<&mut Node, With<HealthBar>>,
        Single<&mut Node, With<StaminaBar>>,
    )>,
    mut vignette: Single<&mut BackgroundColor, With<Vignette>>,
) {
    let st = &local.state;
    let me = local.soldier.and_then(|e| soldiers.get(e).ok());
    let pos = me.map(|(_, tf)| tf.translation).unwrap_or(Vec3::ZERO);
    let yaw = view.yaw + view.look_yaw;
    let hdg = heading(yaw);

    texts.p0().0 = if me.is_some() && !view.map_open {
        format!("{}\n{:03}", compass_tape(hdg), hdg.round() as i32 % 360)
    } else {
        String::new()
    };

    // Nearest enemy-held sector as the current objective.
    let p2 = Vec2::new(pos.x, pos.z);
    let objective = sectors
        .list
        .iter()
        .filter(|s| s.owner == Faction::Opfor)
        .min_by(|a, b| {
            LOCATIONS[a.loc]
                .pos
                .distance(p2)
                .total_cmp(&LOCATIONS[b.loc].pos.distance(p2))
        });
    let held = sectors.list.iter().filter(|s| s.owner == Faction::Blufor).count();
    let mut info = format!(
        "OPERATION IRON RIDGE   sectors {}/{}\n",
        held,
        sectors.list.len()
    );
    if let Some(o) = objective {
        let lp = LOCATIONS[o.loc].pos;
        let d = lp.distance(p2);
        let brg = heading(f32::atan2(-(lp.x - p2.x), -(lp.y - p2.y)));
        info.push_str(&format!("OBJ  {}  {:.1} km  bearing {:03}", LOCATIONS[o.loc].name, d / 1000.0, brg.round() as i32 % 360));
        if o.capture > 0.0 {
            info.push_str(&format!("  capturing {:.0}%", o.capture * 100.0));
        }
        info.push('\n');
    }
    for s in &sectors.list {
        if s.owner == Faction::Blufor && s.capture > 0.0 {
            info.push_str(&format!("{} CONTESTED {:.0}%\n", LOCATIONS[s.loc].name, s.capture * 100.0));
        }
    }
    info.push_str(&format!("GRID {}   K/D {}/{}", grid_ref(pos), st.kills, st.deaths));
    if let NetMode::Client(c) = &*net {
        if c.id.is_none() {
            info.push_str("\nConnecting to server...");
        }
    } else if let NetMode::Host(_) = &*net {
        info.push_str("\nHosting co-op session");
    }
    texts.p1().0 = info;

    // Weapon / vehicle panel.
    let weapon = if !st.alive {
        String::new()
    } else if st.in_vehicle {
        let kind = VehicleKind::from_u8(st.vehicle_kind);
        let mut s = format!("{}\nHULL {:.0}%   {:.0} km/h", vehicle_name(kind, st.vehicle_paint), st.vehicle_hp * 100.0, st.vehicle_speed * 3.6);
        if kind == VehicleKind::Heli {
            s.push_str(&format!("   ALT {:.0} m", st.vehicle_alt));
        }
        if st.driver {
            let w = WeaponKind::from_u8(st.vehicle_weapon);
            s.push_str(&format!(
                "\n{}  {} | {}{}",
                w.stats().name,
                st.vehicle_ammo,
                st.vehicle_mags,
                if st.vehicle_reloading { "  RELOADING" } else { "" }
            ));
        } else {
            s.push_str("\nPassenger");
        }
        s
    } else {
        let kind = WeaponKind::from_u8(st.weapons.get(st.active as usize).copied().unwrap_or(0));
        let mode = if kind.stats().full_auto {
            if st.auto_mode { "AUTO" } else { "SEMI" }
        } else {
            "SINGLE"
        };
        format!(
            "{}\n{} | {} mags   {}\nZERO {} m   GREN {}   FAK {}{}",
            kind.stats().name,
            st.ammo,
            st.mags,
            mode,
            view.zeroing,
            st.grenades,
            st.fak,
            if st.reloading { "\nRELOADING" } else { "" }
        )
    };
    texts.p2().0 = weapon;

    texts.p3().0 = if st.alive {
        format!(
            "{}{}",
            Stance::from_u8(st.stance).label(),
            if st.healing { "   APPLYING FIRST AID" } else { "" }
        )
    } else {
        String::new()
    };
    bars.p0().width = Val::Percent(if st.alive { st.hp.clamp(0.0, 100.0) } else { 0.0 });
    bars.p1().width = Val::Percent(st.stamina.clamp(0.0, 1.0) * 100.0);

    let mut squad = String::new();
    if !st.squad.is_empty() {
        squad.push_str(&format!("{} [{}]   ", st.squad_name, st.order));
        for (i, (name, role, alive, _)) in st.squad.iter().enumerate() {
            let role = Role::from_u8(*role);
            let tag = match role {
                Role::Autorifleman => " (AR)",
                Role::AtSoldier => " (AT)",
                _ => "",
            };
            squad.push_str(&format!(
                "{}{} {}{}{}   ",
                if *alive { "" } else { "[KIA] " },
                i + 1,
                name,
                tag,
                ""
            ));
        }
    }
    texts.p4().0 = if view.map_open { String::new() } else { squad };

    // Interaction prompt.
    let mut prompt = String::new();
    if st.alive && !view.map_open {
        if st.in_vehicle {
            prompt = String::new();
        } else if let Some((s, tf)) = me {
            let _ = s;
            let mut best: Option<(f32, &Vehicle)> = None;
            for (v, vtf) in &vehicles {
                if v.destroyed {
                    continue;
                }
                let half = v.spec().half;
                let local_p = vtf.rotation.inverse() * (tf.translation - v.center(vtf));
                let d = (local_p.abs() - half).max(Vec3::ZERO).length();
                if d < 2.5 && best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, v));
                }
            }
            if let Some((_, v)) = best {
                let enemy = v.crew_faction.is_some_and(|f| f != Faction::Blufor);
                if !enemy {
                    let seat = if v.driver.is_none() { "as driver" } else { "as passenger" };
                    prompt = format!("[F] Get in {} {}", v.name(), seat);
                }
            }
        }
    }
    texts.p5().0 = prompt;

    let big = if sectors.won {
        "MISSION ACCOMPLISHED\nAll sectors secured".to_string()
    } else if me.is_none() && st.respawn_timer <= 0.0 && !st.alive && local.soldier.is_none() {
        if matches!(*net, NetMode::Client(_)) {
            "Joining operation...".into()
        } else {
            String::new()
        }
    } else if !st.alive {
        let roles = [Role::Leader, Role::Autorifleman, Role::AtSoldier, Role::Rifleman].map(Role::label);
        let mut s = String::from("YOU ARE DEAD\n");
        if st.respawn_timer > 0.0 {
            s.push_str(&format!("Redeploy available in {:.0} s\n", st.respawn_timer.ceil()));
        } else {
            s.push_str(&format!("Press ENTER to redeploy at {}\n", LOCATIONS[BASE].name));
        }
        s.push_str("\nRole:  ");
        for (i, r) in roles.iter().enumerate() {
            if i as u8 == view.role {
                s.push_str(&format!("[{}] >{}<   ", i + 1, r));
            } else {
                s.push_str(&format!("[{}] {}   ", i + 1, r));
            }
        }
        s
    } else {
        String::new()
    };
    texts.p6().0 = big;

    let hp_missing = if st.alive { 1.0 - st.hp / 100.0 } else { 0.0 };
    let a = (fb.hurt * 0.35 + hp_missing.max(0.0) * 0.25).min(0.6);
    vignette.0 = Color::srgba(0.55, 0.0, 0.0, a);
}

fn vehicle_name(kind: VehicleKind, paint: u8) -> &'static str {
    match (kind, paint) {
        (VehicleKind::Jeep, 0) => "HMMWV (M2)",
        (VehicleKind::Jeep, _) => "UAZ-469 (DShK)",
        (VehicleKind::Ifv, 0) => "M2A3 Bradley",
        (VehicleKind::Ifv, _) => "BMP-2",
        (VehicleKind::Heli, 0) => "UH-1Y Venom",
        (VehicleKind::Heli, _) => "Mi-8MT",
    }
}

#[allow(clippy::type_complexity)]
fn update_crosshair(
    local: Res<LocalState>,
    view: Res<LocalView>,
    input: Res<crate::player::LocalInput>,
    fb: Res<PlayerFeedback>,
    camera: Single<(&Camera, &Transform), With<MainCamera>>,
    soldiers: Query<&Soldier>,
    vehicles: Query<(&Vehicle, &Transform), Without<MainCamera>>,
    mut nodes: ParamSet<(
        Single<&mut Visibility, With<Crosshair>>,
        Single<(&mut Node, &mut Visibility), With<GunMarker>>,
        Single<&Children, With<HitMarker>>,
    )>,
    mut colors: Query<&mut TextColor>,
) {
    let st = &local.state;
    let in_vehicle = st.in_vehicle && st.driver;
    let ads = input.0.ads && !view.third_person;
    let show_cross = st.alive && !view.map_open && (in_vehicle || !ads) && !(st.in_vehicle && !st.driver);
    **nodes.p0() = if show_cross {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    let (cam, cam_tf) = *camera;
    let mut marker = None;
    if in_vehicle
        && !view.map_open
        && let Some(ve) = local.soldier.and_then(|e| soldiers.get(e).ok()).and_then(|s| s.vehicle)
        && let Ok((v, vtf)) = vehicles.get(ve)
    {
        let (muzzle, dir) = v.barrel(vtf);
        let dist = muzzle.distance(view.aim_point).max(20.0);
        let point = muzzle + dir * dist;
        marker = cam
            .world_to_viewport(&GlobalTransform::from(*cam_tf), point)
            .ok();
    }
    {
        let mut p1 = nodes.p1();
        let (node, vis) = &mut *p1;
        match marker {
            Some(px) => {
                node.left = Val::Px(px.x);
                node.top = Val::Px(px.y);
                **vis = Visibility::Inherited;
            }
            None => **vis = Visibility::Hidden,
        }
    }
    let children: Vec<Entity> = nodes.p2().iter().collect();
    for c in children {
        if let Ok(mut col) = colors.get_mut(c) {
            col.0 = Color::srgba(1.0, 1.0, 1.0, fb.hitmarker);
        }
    }
}

#[allow(clippy::type_complexity)]
fn update_name_tags(
    local: Res<LocalState>,
    view: Res<LocalView>,
    index: Res<NetIndex>,
    net: Res<NetMode>,
    camera: Single<(&Camera, &Transform), With<MainCamera>>,
    soldiers: Query<(&Soldier, &Transform, &Health)>,
    squads: Query<&crate::ai::Squad>,
    mut tags: Query<(&NameTag, &mut Text, &mut Node, &mut Visibility)>,
) {
    let (cam, cam_tf) = *camera;
    let gt = GlobalTransform::from(*cam_tf);
    let members: Vec<Option<Entity>> = if net.is_client() {
        local
            .state
            .squad
            .iter()
            .map(|(_, _, _, id)| index.map.get(id).copied())
            .collect()
    } else {
        local
            .soldier
            .and_then(|e| soldiers.get(e).ok())
            .and_then(|(s, _, _)| s.squad)
            .and_then(|q| squads.get(q).ok())
            .map(|q| q.members.iter().map(|&m| Some(m)).collect())
            .unwrap_or_default()
    };
    for (tag, mut text, mut node, mut vis) in &mut tags {
        let mut shown = false;
        if !view.map_open
            && local.state.alive
            && let Some(Some(e)) = members.get(tag.0 + 1)
            && let Ok((s, tf, h)) = soldiers.get(*e)
            && !h.dead
            && s.vehicle.is_none()
        {
            let head = s.eye(tf.translation) + Vec3::Y * 0.45;
            if head.distance(cam_tf.translation) < 200.0
                && let Ok(px) = cam.world_to_viewport(&gt, head)
            {
                text.0 = format!("{}", tag.0 + 2);
                node.left = Val::Px(px.x - 4.0);
                node.top = Val::Px(px.y - 8.0);
                shown = true;
            }
        }
        *vis = if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_map(
    view: Res<LocalView>,
    local: Res<LocalState>,
    sectors: Res<Sectors>,
    soldiers: Query<(Entity, &Soldier, &Transform, &Health)>,
    vehicles: Query<(&Vehicle, &Transform)>,
    mut root: Single<&mut Visibility, With<MapRoot>>,
    mut markers: Query<(&mut Node, &mut BackgroundColor, &mut Visibility), (With<MapMarker>, Without<MapRoot>)>,
    mut labels: Query<(&SectorLabel, &mut TextColor)>,
) {
    **root = if view.map_open {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    if !view.map_open {
        return;
    }
    let to_px = |p: Vec3| {
        Vec2::new(
            (p.x + MAP_HALF) / (2.0 * MAP_HALF) * MAP_PX,
            (p.z + MAP_HALF) / (2.0 * MAP_HALF) * MAP_PX,
        )
    };
    let mut items: Vec<(Vec2, f32, Color)> = Vec::new();
    for s in &sectors.list {
        let l = &LOCATIONS[s.loc];
        let color = match s.owner {
            Faction::Blufor => Faction::Blufor.map_color(),
            Faction::Opfor => Faction::Opfor.map_color(),
        };
        items.push((to_px(Vec3::new(l.pos.x, 0.0, l.pos.y)), 14.0, color.with_alpha(0.75)));
    }
    let base = LOCATIONS[BASE].pos;
    items.push((to_px(Vec3::new(base.x, 0.0, base.y)), 14.0, Color::srgba(0.1, 0.5, 0.1, 0.8)));
    for (v, tf) in &vehicles {
        let c = if v.destroyed {
            Color::srgb(0.2, 0.2, 0.2)
        } else {
            match v.crew_faction {
                Some(Faction::Blufor) => Faction::Blufor.map_color(),
                Some(Faction::Opfor) => continue,
                None if v.paint == Faction::Blufor => Color::srgb(0.6, 0.7, 1.0),
                None => Color::srgb(0.9, 0.9, 0.9),
            }
        };
        items.push((to_px(tf.translation), 9.0, c));
    }
    for (e, s, tf, h) in &soldiers {
        if h.dead || s.faction != Faction::Blufor || s.vehicle.is_some() {
            continue;
        }
        let me = Some(e) == local.soldier;
        let c = if me {
            Color::srgb(1.0, 0.85, 0.1)
        } else {
            Faction::Blufor.map_color()
        };
        items.push((to_px(tf.translation), if me { 9.0 } else { 5.0 }, c));
    }
    if let Some((_, _, tf, _)) = local.soldier.and_then(|e| soldiers.get(e).ok()) {
        // Facing indicator for the local player.
        let fwd = forward_of_yaw(view.yaw);
        for k in 1..4 {
            items.push((to_px(tf.translation + fwd * (k as f32 * 12.0)), 3.0, Color::srgb(1.0, 0.85, 0.1)));
        }
    }
    let mut it = items.into_iter();
    for (mut node, mut bg, mut vis) in &mut markers {
        match it.next() {
            Some((p, size, c)) => {
                node.left = Val::Px(p.x - size / 2.0);
                node.top = Val::Px(p.y - size / 2.0);
                node.width = Val::Px(size);
                node.height = Val::Px(size);
                bg.0 = c;
                *vis = Visibility::Inherited;
            }
            None => *vis = Visibility::Hidden,
        }
    }
    for (l, mut c) in &mut labels {
        let owner = sectors.list.iter().find(|s| s.loc == l.0).map(|s| s.owner);
        c.0 = match owner {
            Some(Faction::Opfor) => Color::srgb(0.6, 0.0, 0.0),
            Some(Faction::Blufor) => Color::srgb(0.0, 0.15, 0.6),
            None => Color::srgb(0.0, 0.3, 0.0),
        };
    }
}
