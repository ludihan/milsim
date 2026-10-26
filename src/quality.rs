//! Graphics quality presets for running well on weak hardware.
//!
//! The level is picked from the GPU at startup (integrated or software renderers get `Low`), can be
//! forced with `--quality low|medium|high`, and cycled in game with F9. It controls render
//! resolution, anti-aliasing, shadows, fog and draw distances per kind of object.

use bevy::{
    anti_alias::fxaa::Fxaa,
    camera::{RenderTarget, visibility::VisibilityRange},
    light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap},
    pbr::{DistanceFog, FogFalloff},
    prelude::*,
    render::{
        render_resource::{TextureFormat, TextureUsages},
        renderer::RenderAdapterInfo,
    },
    window::{PrimaryWindow, WindowResized},
};

use crate::common::{Phase, Radio, RADIO_SYSTEM, SKY_COLOR};
use crate::player::MainCamera;

#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Quality {
    Low,
    Medium,
    High,
}

impl Quality {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "low" => Some(Quality::Low),
            "medium" | "med" => Some(Quality::Medium),
            "high" => Some(Quality::High),
            _ => None,
        }
    }

    fn next(self) -> Self {
        match self {
            Quality::Low => Quality::Medium,
            Quality::Medium => Quality::High,
            Quality::High => Quality::Low,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Quality::Low => "Low",
            Quality::Medium => "Medium",
            Quality::High => "High",
        }
    }

    /// Fraction of the window resolution the 3D scene is rendered at.
    fn render_scale(self) -> f32 {
        match self {
            Quality::Low => 0.6,
            Quality::Medium => 0.8,
            Quality::High => 1.0,
        }
    }

    /// Distance at which fog fully hides the world; everything is culled beyond it.
    fn view_distance(self) -> f32 {
        match self {
            Quality::Low => 900.0,
            Quality::Medium => 1400.0,
            Quality::High => 1900.0,
        }
    }

    pub fn range(self, class: LodClass) -> f32 {
        let (low, medium, high) = match class {
            LodClass::Tree => (500.0, 900.0, 1500.0),
            LodClass::Bush => (120.0, 250.0, 600.0),
            LodClass::Rock => (250.0, 450.0, 900.0),
            LodClass::BuildingDetail => (150.0, 300.0, 700.0),
            LodClass::SoldierDetail => (35.0, 70.0, 150.0),
            LodClass::VehicleDetail => (80.0, 160.0, 400.0),
        };
        match self {
            Quality::Low => low,
            Quality::Medium => medium,
            Quality::High => high,
        }
    }
}

/// Command-line overrides: the preset (otherwise chosen from the GPU) and the render scale.
#[derive(Resource, Default)]
pub struct QualityOverride {
    pub quality: Option<Quality>,
    pub render_scale: Option<f32>,
}

/// What kind of object an entity is, for distance culling.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum LodClass {
    Tree,
    Bush,
    Rock,
    BuildingDetail,
    SoldierDetail,
    VehicleDetail,
}

/// Full-screen image showing the scaled-down 3D render.
#[derive(Component)]
struct SceneView;

#[derive(Resource)]
struct SceneTarget(Handle<Image>);

/// Multiply a viewport position of the 3D camera by this to get UI (logical) pixels.
#[derive(Resource, Clone, Copy)]
pub struct SceneScale(pub f32);

pub struct QualityPlugin;

impl Plugin for QualityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<QualityOverride>()
            .insert_resource(SceneScale(1.0))
            .add_systems(Startup, setup_scene_target)
            .add_systems(
                Update,
                (pick_quality, cycle_quality, apply_quality, resize_scene_target, apply_lod)
                    .chain()
                    .in_set(Phase::Camera),
            );
    }
}

fn setup_scene_target(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut image = Image::new_target_texture(
        16,
        16,
        TextureFormat::Rgba8Unorm,
        Some(TextureFormat::Rgba8UnormSrgb),
    );
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let handle = images.add(image);
    commands.insert_resource(SceneTarget(handle.clone()));
    // The HUD camera draws the 3D image stretched to the window, then the UI on top.
    commands.spawn((
        Camera2d,
        Camera {
            order: 10,
            ..default()
        },
        IsDefaultUiCamera,
    ));
    commands.spawn((
        SceneView,
        ImageNode::new(handle),
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        GlobalZIndex(-100),
    ));
}

fn pick_quality(
    mut commands: Commands,
    current: Option<Res<Quality>>,
    forced: Res<QualityOverride>,
    adapter: Option<Res<RenderAdapterInfo>>,
    mut waited: Local<u32>,
) {
    if current.is_some() {
        return;
    }
    *waited += 1;
    let detected = adapter.map(|a| {
        let kind = format!("{:?}", a.device_type);
        let weak = kind.contains("Integrated")
            || kind.contains("Cpu")
            || a.name.to_lowercase().contains("llvmpipe");
        info!("GPU: {} ({:?})", a.name, a.device_type);
        if weak { Quality::Low } else { Quality::High }
    });
    let q = match (forced.quality, detected) {
        (Some(q), _) => q,
        (None, Some(q)) => q,
        (None, None) if *waited > 30 => Quality::Low,
        _ => return,
    };
    info!("Graphics quality: {} (F9 to change)", q.name());
    commands.insert_resource(q);
}

fn cycle_quality(
    keys: Res<ButtonInput<KeyCode>>,
    quality: Option<ResMut<Quality>>,
    mut radio: MessageWriter<Radio>,
) {
    let Some(mut q) = quality else { return };
    if keys.just_pressed(KeyCode::F9) {
        *q = q.next();
        radio.write(Radio::new(format!("Graphics quality: {}", q.name()), RADIO_SYSTEM));
    }
}

#[allow(clippy::type_complexity)]
fn apply_quality(
    mut commands: Commands,
    quality: Option<Res<Quality>>,
    mut cameras: Query<
        (Entity, &mut DistanceFog, &mut Projection, &mut Msaa),
        With<MainCamera>,
    >,
    mut view_cams: Query<&mut Msaa, (With<Camera3d>, Without<MainCamera>)>,
    mut suns: Query<(Entity, &mut DirectionalLight)>,
) {
    let Some(q) = quality else { return };
    if !q.is_changed() {
        return;
    }
    let q = *q;
    let far = q.view_distance();
    for (e, mut fog, mut proj, mut msaa) in &mut cameras {
        fog.falloff = FogFalloff::Linear {
            start: far * 0.15,
            end: far,
        };
        fog.color = SKY_COLOR;
        if let Projection::Perspective(p) = &mut *proj {
            p.far = far + 100.0;
        }
        *msaa = if q == Quality::High {
            Msaa::Sample4
        } else {
            Msaa::Off
        };
        if q == Quality::Medium {
            commands.entity(e).insert(Fxaa::default());
        } else {
            commands.entity(e).remove::<Fxaa>();
        }
    }
    for mut msaa in &mut view_cams {
        *msaa = if q == Quality::High {
            Msaa::Sample4
        } else {
            Msaa::Off
        };
    }
    for (e, mut sun) in &mut suns {
        sun.shadow_maps_enabled = q != Quality::Low;
        let cascades = match q {
            Quality::High => CascadeShadowConfigBuilder {
                num_cascades: 3,
                first_cascade_far_bound: 25.0,
                maximum_distance: 220.0,
                ..default()
            },
            _ => CascadeShadowConfigBuilder {
                num_cascades: 1,
                maximum_distance: 70.0,
                ..default()
            },
        };
        commands.entity(e).insert(cascades.build());
    }
    commands.insert_resource(DirectionalLightShadowMap {
        size: if q == Quality::High { 2048 } else { 1024 },
    });
}

/// Keep the off-screen scene image at the window size times the render scale.
#[allow(clippy::type_complexity)]
fn resize_scene_target(
    quality: Option<Res<Quality>>,
    target: Res<SceneTarget>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut resized: MessageReader<WindowResized>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Query<&mut RenderTarget, With<Camera3d>>,
    mut last: Local<UVec2>,
    mut scene_scale: ResMut<SceneScale>,
    forced: Res<QualityOverride>,
) {
    let Some(q) = quality else { return };
    let _ = resized.read().count();
    let scale = forced.render_scale.unwrap_or(q.render_scale()).clamp(0.25, 2.0);
    let size = (Vec2::new(window.physical_width() as f32, window.physical_height() as f32) * scale)
        .max(Vec2::splat(16.0))
        .as_uvec2();
    if size == *last {
        return;
    }
    *last = size;
    scene_scale.0 = window.width() / size.x as f32;
    if let Some(mut image) = images.get_mut(&target.0) {
        image.resize(bevy::render::render_resource::Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        });
    }
    for mut rt in &mut cameras {
        *rt = RenderTarget::Image(target.0.clone().into());
    }
}

/// Give every LOD-classed entity the draw distance for the current quality.
fn apply_lod(
    mut commands: Commands,
    quality: Option<Res<Quality>>,
    all: Query<(Entity, &LodClass)>,
    added: Query<(Entity, &LodClass), Added<LodClass>>,
) {
    let Some(q) = quality else { return };
    let iter: Box<dyn Iterator<Item = (Entity, &LodClass)>> = if q.is_changed() {
        Box::new(all.iter())
    } else {
        Box::new(added.iter())
    };
    for (e, class) in iter {
        commands
            .entity(e)
            .insert(VisibilityRange::abrupt(0.0, q.range(*class)));
    }
}
