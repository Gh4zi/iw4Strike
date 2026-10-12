use std::sync::{Arc, Mutex};

use bevy::mesh::VertexBufferLayout;
use bevy::platform::collections::HashMap;
use bevy::prelude::Resource;
use bevy::render::render_resource::{
    BindGroupLayout, BindGroupLayoutDescriptor, ColorTargetState, DepthStencilState,
    MultisampleState, PipelineCompilationOptions, PipelineLayoutDescriptor, PrimitiveState,
    RawFragmentState, RawRenderPipelineDescriptor, RawVertexBufferLayout, RawVertexState,
    RenderPipeline, ShaderModule, ShaderModuleDescriptor, ShaderSource,
};
use bevy::render::renderer::RenderDevice;
use bevy::tasks::{Task, futures_lite::future};

use super::sm3_wgsl::{PASS_VERTEX_ENTRY, ValidatedPassWgsl};
use render_material::PortId;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct ModuleKey {
    port: PortId,
    cached_lighting: bool,
}

#[derive(Clone)]
pub(super) enum ExactModuleSource {
    Port(Arc<ValidatedPassWgsl>),
    CachedLighting(Arc<str>),
}

impl ExactModuleSource {
    fn wgsl(&self) -> &str {
        match self {
            Self::Port(module) => &module.source,
            Self::CachedLighting(source) => source,
        }
    }
}

pub(super) struct ExactPipelinePlan {
    pub(super) label: String,
    pub(super) fragment_entry: String,
    pub(super) vertex_buffers: Vec<VertexBufferLayout>,
    pub(super) targets: Vec<Option<ColorTargetState>>,
    pub(super) primitive: PrimitiveState,
    pub(super) depth_stencil: DepthStencilState,
    pub(super) multisample: MultisampleState,
    pub(super) constants_layout: BindGroupLayout,
    pub(super) textures_layout: BindGroupLayout,
    /// What the two layouts were made from (constants, textures): the port's SPIR-V binds by
    /// them.
    pub(super) binding_layouts: [BindGroupLayoutDescriptor; 2],
}

/// Where exact ports can go to the driver as SPIR-V of our own: a Vulkan device with
/// `PASSTHROUGH_SHADERS`, and the features and downlevel flags wgpu validates WGSL against.
#[derive(Clone, Copy)]
pub(super) struct SpirvTarget {
    features: bevy::render::render_resource::WgpuFeatures,
    downlevel: bevy::render::render_resource::DownlevelFlags,
}

impl SpirvTarget {
    /// `IW4L_SPIRV_PASSTHROUGH=0` keeps exact ports on wgpu's own WGSL path.
    pub(super) fn of(
        device: &RenderDevice,
        adapter: &bevy::render::renderer::RenderAdapter,
        info: &bevy::render::renderer::RenderAdapterInfo,
    ) -> Option<Self> {
        static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let off = *OFF
            .get_or_init(|| std::env::var("IW4L_SPIRV_PASSTHROUGH").is_ok_and(|v| v.trim() == "0"));
        let features = device.features();
        (!off
            && info.backend == wgpu::Backend::Vulkan
            && features.contains(bevy::render::render_resource::WgpuFeatures::PASSTHROUGH_SHADERS))
        .then(|| Self {
            features,
            downlevel: adapter.get_downlevel_capabilities().flags,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub(super) struct ExactPipelineSlot(u32);

enum SlotState {
    Building,
    Ready(RenderPipeline),
}

struct PortBuild {
    module: ModuleKey,
    created: Option<Arc<ShaderModule>>,
    pipelines: Vec<(ExactPipelineSlot, RenderPipeline)>,
}

#[derive(Resource, Default)]
pub(super) struct ExactPipelineRegistry {
    slots: Vec<SlotState>,
    by_key: HashMap<super::colour_submit::ExactColourPipelineKey, ExactPipelineSlot>,
    modules: HashMap<ModuleKey, Arc<ShaderModule>>,

    queued: HashMap<
        ModuleKey,
        (
            ExactModuleSource,
            Vec<(ExactPipelineSlot, ExactPipelinePlan)>,
        ),
    >,
    jobs: Vec<Task<PortBuild>>,

    layouts: Mutex<HashMap<BindGroupLayoutDescriptor, BindGroupLayout>>,

    discovered: Mutex<Vec<super::colour_submit::ExactColourPipelineKey>>,
}

impl ExactPipelineRegistry {
    pub(super) fn bind_group_layout(
        &self,
        device: &RenderDevice,
        descriptor: &BindGroupLayoutDescriptor,
    ) -> BindGroupLayout {
        self.layouts
            .lock()
            .expect("exact pipeline layout cache is never poisoned")
            .entry(descriptor.clone())
            .or_insert_with_key(|descriptor| {
                device.create_bind_group_layout(descriptor.label.as_ref(), &descriptor.entries)
            })
            .clone()
    }

    pub(super) fn discover(&self, key: super::colour_submit::ExactColourPipelineKey) {
        self.discovered
            .lock()
            .expect("exact pipeline discovery list is never poisoned")
            .push(key);
    }

    pub(super) fn take_discovered(&mut self) -> Vec<super::colour_submit::ExactColourPipelineKey> {
        std::mem::take(
            &mut *self
                .discovered
                .lock()
                .expect("exact pipeline discovery list is never poisoned"),
        )
    }

    pub(super) fn slot(
        &self,
        key: &super::colour_submit::ExactColourPipelineKey,
    ) -> Option<ExactPipelineSlot> {
        self.by_key.get(key).copied()
    }

    pub(super) fn ready(&self, slot: ExactPipelineSlot) -> Option<&RenderPipeline> {
        match self.slots.get(slot.0 as usize) {
            Some(SlotState::Ready(pipeline)) => Some(pipeline),
            _ => None,
        }
    }

    pub(super) fn is_ready(&self, slot: ExactPipelineSlot) -> bool {
        matches!(self.slots.get(slot.0 as usize), Some(SlotState::Ready(_)))
    }

    pub(super) fn request(
        &mut self,
        key: super::colour_submit::ExactColourPipelineKey,
        source: ExactModuleSource,
        plan: ExactPipelinePlan,
    ) -> ExactPipelineSlot {
        if let Some(slot) = self.by_key.get(&key).copied() {
            return slot;
        }
        let slot = ExactPipelineSlot(
            u32::try_from(self.slots.len()).expect("exact pipeline slot count fits u32"),
        );
        self.slots.push(SlotState::Building);
        self.by_key.insert(key, slot);

        let module = ModuleKey {
            port: key.port,
            cached_lighting: matches!(source, ExactModuleSource::CachedLighting(_)),
        };
        self.queued
            .entry(module)
            .or_insert_with(|| (source, Vec::new()))
            .1
            .push((slot, plan));
        slot
    }

    pub(super) fn flush(&mut self, device: &RenderDevice, spirv: Option<SpirvTarget>) {
        if self.queued.is_empty() {
            return;
        }

        let pool = assets::load_pool();
        for (module, (source, plans)) in self.queued.drain() {
            let device = device.clone();
            let existing = self.modules.get(&module).cloned();
            self.jobs.push(
                pool.spawn(
                    async move { build_port(&device, module, source, existing, plans, spirv) },
                ),
            );
        }
    }

    pub(super) fn poll(&mut self) {
        let mut index = 0;
        while index < self.jobs.len() {
            let Some(build) = future::block_on(future::poll_once(&mut self.jobs[index])) else {
                index += 1;
                continue;
            };

            drop(self.jobs.swap_remove(index));
            if let Some(created) = build.created {
                self.modules.insert(build.module, created);
            }
            for (slot, pipeline) in build.pipelines {
                self.slots[slot.0 as usize] = SlotState::Ready(pipeline);
            }
        }
    }

    pub(super) fn module_n(&self) -> usize {
        self.modules.len()
    }

    pub(super) fn building_n(&self) -> usize {
        self.jobs.len()
    }
}

fn build_port(
    device: &RenderDevice,
    module: ModuleKey,
    source: ExactModuleSource,
    existing: Option<Arc<ShaderModule>>,
    plans: Vec<(ExactPipelineSlot, ExactPipelinePlan)>,
    spirv: Option<SpirvTarget>,
) -> PortBuild {
    let label = format!(
        "iw4_exact_colour/{:016x}/{}{}",
        module.port.vertex_program_hash,
        module.port.vertex_type,
        if module.cached_lighting {
            "/cached"
        } else {
            ""
        }
    );

    let (shader, created) = match existing {
        Some(shader) => (shader, None),
        None => {
            let own = spirv.zip(plans.first()).map(|(target, (_, plan))| {
                port_spirv(source.wgsl(), &plan.binding_layouts, target)
            });
            let shader = Arc::new(match own {
                // SAFETY: SPIR-V naga wrote from WGSL it validated against this device's
                // features, binding the layouts every pipeline of this port is made with.
                Some(Ok(words)) => unsafe {
                    device.wgpu_device().create_shader_module_passthrough(
                        wgpu::ShaderModuleDescriptorPassthrough {
                            label: Some(&label),
                            spirv: Some(std::borrow::Cow::Owned(words)),
                            ..Default::default()
                        },
                    )
                },
                own => {
                    if let Some(Err(error)) = own {
                        diag::warn!(World, "{label}: own SPIR-V failed, WGSL instead: {error}");
                    }
                    unsafe {
                        device.create_shader_module(ShaderModuleDescriptor {
                            label: Some(&label),
                            source: ShaderSource::Wgsl(std::borrow::Cow::Borrowed(source.wgsl())),
                        })
                    }
                }
            });
            (shader.clone(), Some(shader))
        }
    };
    let pipelines = plans
        .into_iter()
        .map(|(slot, plan)| (slot, build_pipeline(device, &shader, &plan)))
        .collect();
    PortBuild {
        module,
        created,
        pipelines,
    }
}

fn build_pipeline(
    device: &RenderDevice,
    shader: &ShaderModule,
    plan: &ExactPipelinePlan,
) -> RenderPipeline {
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some(&plan.label),
        bind_group_layouts: &[Some(&plan.constants_layout), Some(&plan.textures_layout)],
        immediate_size: 0,
    });
    let buffers: Vec<RawVertexBufferLayout> = plan
        .vertex_buffers
        .iter()
        .map(|buffer| RawVertexBufferLayout {
            array_stride: buffer.array_stride,
            attributes: &buffer.attributes,
            step_mode: buffer.step_mode,
        })
        .collect();
    let compilation_options = PipelineCompilationOptions {
        constants: &[],
        zero_initialize_workgroup_memory: false,
    };
    device.create_render_pipeline(&RawRenderPipelineDescriptor {
        label: Some(&plan.label),
        layout: Some(&layout),
        vertex: RawVertexState {
            module: shader,
            entry_point: Some(PASS_VERTEX_ENTRY),
            buffers: &buffers,
            compilation_options: compilation_options.clone(),
        },
        fragment: Some(RawFragmentState {
            module: shader,
            entry_point: Some(&plan.fragment_entry),
            targets: &plan.targets,
            compilation_options,
        }),
        primitive: plan.primitive,
        depth_stencil: Some(plan.depth_stencil.clone()),
        multisample: plan.multisample,
        multiview_mask: None,
        cache: None,
    })
}

/// An exact port's WGSL as SPIR-V, written the way wgpu-hal's Vulkan backend writes a WGSL
/// module created with unchecked runtime checks (what `RenderDevice::create_shader_module`
/// asks for): the same validator, each group's entries in binding order taking dense Vulkan
/// bindings, no bounds checks, no loop bounding, varyings labelled, point size forced.
///
/// Handed to the driver as is, the port leaves wgpu nothing to keep: a WGSL module keeps its
/// naga IR and validation info for as long as any pipeline made from it lives, ~1.4 GB over
/// Favela's ~5000 ports.
fn port_spirv(
    wgsl: &str,
    layouts: &[BindGroupLayoutDescriptor; 2],
    target: SpirvTarget,
) -> Result<Vec<u32>, String> {
    use naga::back::spv;
    use naga::proc::{BoundsCheckPolicies, BoundsCheckPolicy};

    let module = naga::front::wgsl::parse_str(wgsl).map_err(|error| error.to_string())?;
    let info = wgpu_naga_bridge::create_validator(
        target.features,
        target.downlevel,
        naga::valid::ValidationFlags::all(),
    )
    .validate(&module)
    .map_err(|error| format!("{error:?}"))?;
    let mut binding_map = spv::BindingMap::default();
    for (group, layout) in layouts.iter().enumerate() {
        let mut entries: Vec<_> = layout.entries.iter().collect();
        entries.sort_by_key(|entry| entry.binding);
        for (dense, entry) in entries.into_iter().enumerate() {
            binding_map.insert(
                naga::ResourceBinding {
                    group: group as u32,
                    binding: entry.binding,
                },
                spv::BindingInfo {
                    descriptor_set: group as u32,
                    binding: dense as u32,
                    binding_array_size: entry.count.map(std::num::NonZeroU32::get),
                },
            );
        }
    }
    let options = spv::Options {
        // The newest version every Vulkan 1.1 device reads; wgpu picks up to 1.6 by device.
        lang_version: (1, 3),
        flags: spv::WriterFlags::LABEL_VARYINGS | spv::WriterFlags::FORCE_POINT_SIZE,
        fake_missing_bindings: false,
        binding_map,
        capabilities: None,
        bounds_check_policies: BoundsCheckPolicies {
            index: BoundsCheckPolicy::Unchecked,
            buffer: BoundsCheckPolicy::Unchecked,
            image_load: BoundsCheckPolicy::Unchecked,
            binding_array: BoundsCheckPolicy::Unchecked,
        },
        zero_initialize_workgroup_memory: spv::ZeroInitializeWorkgroupMemoryMode::None,
        force_loop_bounding: false,
        ray_query_initialization_tracking: false,
        use_storage_input_output_16: false,
        debug_info: None,
        task_dispatch_limits: None,
        mesh_shader_primitive_indices_clamp: false,
    };
    spv::write_vec(&module, &info, &options, None).map_err(|error| error.to_string())
}
