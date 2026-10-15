use assets::AssetPlugin;
use audio::AudioPlugin;
use bevy::{
    log::LogPlugin,
    prelude::*,
    render::{
        RenderPlugin as BevyRenderPlugin,
        pipelined_rendering::PipelinedRenderingPlugin,
        settings::{RenderCreation, WgpuFeatures, WgpuSettings},
    },
};
use bots::BotsPlugin;
use console::ConsolePlugin;
use frame::RuntimeRole;
use hud::HudPlugin;
use net::NetPlugin;
use render::RenderPlugin;
use replay::ReplayPlugin;
use session::SessionPlugin;
use ui::UiPlugin;

pub fn add_runtime_plugins(app: &mut App) {
    add_runtime_plugins_with_role(app, RuntimeRole::Listen);
}

pub fn add_runtime_plugins_with_role(app: &mut App, role: RuntimeRole) {
    let net = match role {
        RuntimeRole::Listen => NetPlugin::listen(),
        RuntimeRole::Dedicated => NetPlugin::dedicated(),
        RuntimeRole::Client => NetPlugin::client(),
        RuntimeRole::Replay => NetPlugin::replay(),
    };
    if crate::bench::enabled() {
        // Unfocused benchmarks must not inherit the window runner's 60 Hz sleep.
        app.insert_resource(bevy::winit::WinitSettings::continuous());
    }
    app.insert_resource(audio::AudioRuntime::new(
        role != RuntimeRole::Dedicated && !audio::AudioSilent::active(),
    ));
    app.add_plugins(AssetPlugin)
        .add_plugins(UiPlugin)
        .add_plugins(ConsolePlugin)
        .add_plugins(net)
        .add_plugins((BotsPlugin, HudPlugin))
        .add_plugins(AudioPlugin)
        .add_plugins(ReplayPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(SessionPlugin);

    // Bevy lays the whole UI tree out, and rebuilds its draw order, every frame. With none of
    // what they read changed, that is last frame's result again, and walking the HUD's tree for
    // them was 3-7% of the main thread. `IW4L_UI_LAYOUT_ALWAYS=1` runs both every frame anyway.
    if !ui_layout_always() {
        app.configure_sets(
            PostUpdate,
            (
                bevy::ui::UiSystems::Layout.run_if(ui_layout_needed),
                bevy::ui::UiSystems::Stack.run_if(ui_stack_needed),
            ),
        );
    }

    app.edit_schedule(Update, |schedule| {
        schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
    });

    app.edit_schedule(First, |schedule| {
        schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
    });
    app.edit_schedule(PreUpdate, |schedule| {
        schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
    });
    app.edit_schedule(PostUpdate, |schedule| {
        schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
    });
    app.edit_schedule(Last, |schedule| {
        schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
    });
    // The publish step's systems are small and run while the main thread waits on them; on the
    // multi-threaded executor it spent that time on each hop to a pool thread and back. The fixed
    // schedules stay multi-threaded: there the bots think on a pool thread beside the rest.
    app.edit_schedule(frame::AuthorityPublish, |schedule| {
        schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
    });

    if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
        render_app.edit_schedule(bevy::render::renderer::RenderGraph, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        render_app.edit_schedule(bevy::core_pipeline::Core3d, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        render_app.edit_schedule(bevy::core_pipeline::Core2d, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        render_app.edit_schedule(bevy::render::Render, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        render_app.edit_schedule(bevy::render::ExtractSchedule, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
    }
}

/// What `bevy::ui::ui_layout_system` reads, besides measured content: node styles, render
/// targets, UI transforms, scrolling, outlines, layout configs and the UI hierarchy.
type UiLayoutInputsChanged = Or<(
    Changed<bevy::ui::Node>,
    Changed<bevy::ui::ComputedUiRenderTargetInfo>,
    Changed<bevy::ui::UiTransform>,
    Changed<bevy::ui::ScrollPosition>,
    Changed<bevy::ui::Outline>,
    Changed<bevy::ui::LayoutConfig>,
    Changed<bevy::ui::IgnoreScroll>,
    Changed<Children>,
    Changed<ChildOf>,
)>;

/// Whether the UI needs laying out this frame: anything layout reads changed, or a UI node, a
/// child list or a parent link went away. Measured content counts when it changed, except an
/// editable text's: Bevy's own text-input layout takes every `EditableText` mutably each frame
/// and its content-size system then sets the same size again, which alone laid the whole UI out
/// every frame (the closed console's prompt). That size follows only the text's font, line
/// height and render target, which are watched instead. Logs how often layout was needed, at
/// debug level.
fn ui_layout_needed(
    changed: Query<(), UiLayoutInputsChanged>,
    content: Query<(), (Changed<bevy::ui::ContentSize>, Without<bevy::text::EditableText>)>,
    editable: Query<
        (),
        (
            With<bevy::text::EditableText>,
            Or<(
                Added<bevy::ui::ContentSize>,
                Changed<bevy::text::TextFont>,
                Changed<bevy::text::LineHeight>,
            )>,
        ),
    >,
    mut removed_nodes: RemovedComponents<bevy::ui::Node>,
    mut removed_children: RemovedComponents<Children>,
    mut removed_parents: RemovedComponents<ChildOf>,
    mut census: Local<(u32, u32)>,
) -> bool {
    // Every reader drained every frame, so a removal never waits for a later frame to count.
    let removed = (removed_nodes.read().count()
        + removed_children.read().count()
        + removed_parents.read().count())
        > 0;
    let needed = removed || !changed.is_empty() || !content.is_empty() || !editable.is_empty();
    let (laid_out, skipped) = &mut *census;
    if needed {
        *laid_out += 1;
    } else {
        *skipped += 1;
    }
    if *laid_out + *skipped >= 2000 {
        diag::debug!(Ui, "ui layout: {laid_out} frames laid out, {skipped} skipped (unchanged)");
        *census = (0, 0);
    }
    needed
}

/// What `bevy::ui::UiSystems::Stack` (the UI's draw order) reads: z-indexes and the UI tree.
/// A new node arrives with a fresh `ComputedStackIndex`.
type UiStackInputsChanged = Or<(
    Added<bevy::ui::ComputedStackIndex>,
    Changed<bevy::ui::ZIndex>,
    Changed<bevy::ui::GlobalZIndex>,
    Changed<Children>,
    Changed<ChildOf>,
)>;

/// Whether the UI's draw order needs rebuilding this frame: a z-index or the UI tree changed, or
/// a node, a z-index, a child list or a parent link went away.
fn ui_stack_needed(
    changed: Query<(), UiStackInputsChanged>,
    mut removed_nodes: RemovedComponents<bevy::ui::Node>,
    mut removed_z: RemovedComponents<bevy::ui::ZIndex>,
    mut removed_global_z: RemovedComponents<bevy::ui::GlobalZIndex>,
    mut removed_children: RemovedComponents<Children>,
    mut removed_parents: RemovedComponents<ChildOf>,
) -> bool {
    // Every reader drained every frame, so a removal never waits for a later frame to count.
    let removed = (removed_nodes.read().count()
        + removed_z.read().count()
        + removed_global_z.read().count()
        + removed_children.read().count()
        + removed_parents.read().count())
        > 0;
    removed || !changed.is_empty()
}

fn ui_layout_always() -> bool {
    std::env::var("IW4L_UI_LAYOUT_ALWAYS").is_ok_and(|value| value.trim() == "1")
}

pub fn assemble_listen_app() -> App {
    let mut app = App::new();
    app.add_plugins(default_plugins_with_quiet_log(WindowPlugin {
        primary_window: None,
        ..default()
    }));
    add_runtime_plugins(&mut app);
    app
}

pub fn default_plugins_with_quiet_log(mut window: WindowPlugin) -> bevy::app::PluginGroupBuilder {
    if let Some(primary) = window.primary_window.as_mut() {
        primary.desired_maximum_frame_latency = core::num::NonZeroU32::new(frame_latency());
    }
    let mut wgpu = WgpuSettings {
        // Smaller GPU memory blocks: the default keeps 64-128 MiB host-visible blocks around
        // after the load's uploads (~200 MB on Favela) on machines already short of RAM, and BR
        // frame times did not move.
        memory_hints: bevy::render::settings::MemoryHints::MemoryUsage,
        ..default()
    };
    wgpu.features |= WgpuFeatures::TEXTURE_FORMAT_16BIT_NORM
        | WgpuFeatures::TEXTURE_COMPRESSION_BC
        | WgpuFeatures::POLYGON_MODE_LINE
        | WgpuFeatures::TEXTURE_BINDING_ARRAY
        | WgpuFeatures::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING
        | WgpuFeatures::PARTIALLY_BOUND_BINDING_ARRAY
        // Exact material ports go to Vulkan as SPIR-V of our own (render_gpu `exact_pipeline`).
        // Vulkan, D3D12 and Metal all offer it; only Vulkan gets SPIR-V.
        | WgpuFeatures::PASSTHROUGH_SHADERS;
    let plugins = DefaultPlugins
        .set(window)
        .set(LogPlugin {
            filter: "warn,iw4l=info".into(),
            level: bevy::log::Level::WARN,
            #[cfg(feature = "bevy-profile")]
            custom_layer: crate::system_profile::layer,
            ..default()
        })
        .set(BevyRenderPlugin {
            render_creation: RenderCreation::Automatic(Box::new(wgpu)),
            ..default()
        });
    if pipelined_rendering() {
        plugins
    } else {
        plugins.disable::<PipelinedRenderingPlugin>()
    }
}

const PIPELINED_RENDERING_ENV: &str = "IW4L_PIPELINED_RENDERING";

/// Overlap rendering with the next main frame. Extraction remains the ownership
/// boundary; the bounded render channel permits one outstanding frame.
/// Set IW4L_PIPELINED_RENDERING=0 for synchronous presentation.
///
/// Off by default on macOS: AppKit only lets the main thread touch the NSView
/// behind the Metal surface. Bevy hands `create_surfaces` back to the main
/// thread through the multi-threaded executor, which the single-threaded
/// `Render` schedule above bypasses, so the render thread would create it and
/// panic in `raw-window-metal`.
fn pipelined_rendering() -> bool {
    match std::env::var_os(PIPELINED_RENDERING_ENV) {
        None => !cfg!(target_os = "macos"),
        Some(_) => perf::switch(PIPELINED_RENDERING_ENV),
    }
}

const FRAME_LATENCY_ENV: &str = "IW4L_FRAME_LATENCY";

/// How many frames the surface is asked to let the CPU run ahead of the GPU.
///
/// wgpu calls this a hint and the backend is free to clamp it: on Vulkan it is
/// bound to the number of swapchain images, so a run that asked for two did
/// not necessarily get two, and only a measurement says which. The player picks
/// it (`max_frames_ahead` in settings.cfg, Advanced Video in the menu); the
/// variable overrides that so a paired run needs no settings file, and the
/// manifest records the number that was asked for — never the number the driver
/// granted, which this process cannot read back.
///
/// Two is the default: with one, the render thread waits on the GPU for the
/// previous frame at every swap-chain acquire (~1.2 ms a frame on a CPU-bound
/// run), and a second frame only queues when the GPU is the slower side.
///
/// The value is a count, not a switch: anything unparseable or zero is the
/// default, and says so rather than silently picking an arm. Read once, so the
/// window and the manifest cannot disagree and the complaint is made once.
pub(crate) fn frame_latency() -> u32 {
    const DEFAULT: u32 = frame::GameSettings::MAX_FRAMES_AHEAD_DEFAULT as u32;
    static FRAMES: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *FRAMES.get_or_init(|| {
        let Some(asked) = std::env::var_os(FRAME_LATENCY_ENV) else {
            return asset_transport::game_paths::saved_setting("max_frames_ahead")
                .and_then(|value| value.parse::<u32>().ok())
                .map_or(DEFAULT, |frames| frames.clamp(1, 2));
        };
        match asked.to_str().map(str::trim).and_then(|v| v.parse().ok()) {
            Some(frames) if frames > 0 => frames,
            _ => {
                diag::warn!(
                    Launch,
                    "{FRAME_LATENCY_ENV}={} is not a frame count; using {DEFAULT}",
                    asked.to_string_lossy(),
                );
                DEFAULT
            }
        }
    })
}
