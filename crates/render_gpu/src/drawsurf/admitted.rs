use std::sync::Arc;

use d3d9_sm3::PackedPassWgsl;
use render_frame::WgpuPassLayout;
use render_material::{
    ConstantSource, ExecutablePassView, PackedCodeConstantLane, PassProgramAbi, PortId,
    RuntimeShaderStage, is_frame_shared_code_constant,
};

use super::gpu_prepare::{
    ConstantPackRefusal, PassConstantBuffers, overlay_lane_bytes, pack_code_on_seed,
    require_abi_rows_with_lanes,
};

#[derive(Clone, Debug)]
pub struct AdmittedExactPort {
    pub id: PortId,
    pub abi: PassProgramAbi,
    pub module: Arc<PackedPassWgsl>,
    pub layout: WgpuPassLayout,
}

fn frame_shared_bindings(
    bindings: &[render_material::ConstantBinding],
    on: bool,
) -> impl Iterator<Item = u16> + '_ {
    bindings
        .iter()
        .filter(move |binding| {
            on && matches!(binding.source, ConstantSource::Code { index, .. }
                if is_frame_shared_code_constant(index))
        })
        .map(|binding| binding.register)
}

impl AdmittedExactPort {
    pub fn id(&self) -> PortId {
        self.id
    }

    pub fn abi(&self) -> &PassProgramAbi {
        &self.abi
    }

    pub fn module(&self) -> &PackedPassWgsl {
        &self.module
    }

    pub fn shared_module(&self) -> Arc<PackedPassWgsl> {
        Arc::clone(&self.module)
    }

    pub fn wgpu_layout(&self) -> &WgpuPassLayout {
        &self.layout
    }

    /// Whether the stage reads its frame-shared code constants through row indices (see
    /// `render_material::FRAME_SHARED_CODE_CONSTANTS`).
    pub fn frame_shared(&self, stage: RuntimeShaderStage) -> bool {
        match stage {
            RuntimeShaderStage::Vertex => self.module.vertex_frame_shared,
            RuntimeShaderStage::Pixel => self.module.pixel_frame_shared,
        }
    }

    pub fn any_frame_shared(&self) -> bool {
        self.module.vertex_frame_shared || self.module.pixel_frame_shared
    }

    /// The lane's rows hold row indices in the packed span, not its values.
    pub fn frame_shared_lane(&self, lane: &PackedCodeConstantLane) -> bool {
        is_frame_shared_code_constant(lane.index) && self.frame_shared(lane.stage)
    }

    /// The registers read through row indices, as (pixel stage, register).
    pub fn frame_shared_registers(&self) -> impl Iterator<Item = (bool, u16)> + '_ {
        frame_shared_bindings(&self.abi.vertex_constants, self.module.vertex_frame_shared)
            .map(|register| (false, register))
            .chain(
                frame_shared_bindings(&self.abi.pixel_constants, self.module.pixel_frame_shared)
                    .map(|register| (true, register)),
            )
    }

    /// For a span with no frame table: appends each frame-shared register's value after the
    /// span and points the register at it. `span` starts at the span, which sits at `span_row`
    /// of the bound buffer.
    pub fn append_inline_frame_rows(&self, span: &mut Vec<u8>, span_row: u32) {
        let vertex_len = self.module.vertex_constant_len;
        for (pixel, register) in self.frame_shared_registers() {
            let at = (if pixel { vertex_len } else { 0 } + usize::from(register)) * 16;
            let Some(value) = span.get(at..at + 16).map(<[u8]>::to_vec) else {
                continue;
            };
            let row = span_row
                .saturating_add(u32::try_from(span.len() / 16).expect("constant row fits u32"));
            span.extend_from_slice(&value);
            span[at..at + 16].copy_from_slice(bytemuck::cast_slice(&[row, 0, 0, 0]));
        }
    }

    pub fn inline_frame_row_count(&self) -> usize {
        self.frame_shared_registers().count()
    }

    pub fn accepts_pass(&self, pass: ExecutablePassView<'_>) -> bool {
        pass.port == self.id && pass.port.matches_shader_pair(pass.shader_pair)
    }

    pub fn pack_hit(
        &self,
        pass: ExecutablePassView<'_>,
    ) -> Result<PassConstantBuffers, ConstantPackRefusal> {
        let Some(seed) = pass.local_banks.map(Arc::as_ref) else {
            return Err(ConstantPackRefusal::MissingLocalBanks);
        };
        if seed.vertex.len() == self.module.vertex_constant_len
            && seed.pixel.len() == self.module.pixel_constant_len
            && seed.written_vertex.len() == seed.vertex.len()
            && seed.written_pixel.len() == seed.pixel.len()
        {
            return pack_code_on_seed(
                self.id,
                &self.abi.vertex_constants,
                &self.abi.pixel_constants,
                seed,
                pass,
            );
        }
        Err(ConstantPackRefusal::DestinationPastConstantBlock {
            stage: RuntimeShaderStage::Vertex,
            destination: 0,
            block_len: seed.vertex.len(),
        })
    }

    pub fn pack_hit_into(
        &self,
        pass: ExecutablePassView<'_>,
        overlay: &[PackedCodeConstantLane],
        out: &mut Vec<u8>,
    ) -> Result<(), ConstantPackRefusal> {
        let Some(seed) = pass.local_banks.map(Arc::as_ref) else {
            return Err(ConstantPackRefusal::MissingLocalBanks);
        };
        if seed.vertex.len() != self.module.vertex_constant_len
            || seed.pixel.len() != self.module.pixel_constant_len
            || seed.written_vertex.len() != seed.vertex.len()
            || seed.written_pixel.len() != seed.pixel.len()
        {
            return Err(ConstantPackRefusal::DestinationPastConstantBlock {
                stage: RuntimeShaderStage::Vertex,
                destination: 0,
                block_len: seed.vertex.len(),
            });
        }
        if !self.accepts_pass(pass) {
            return Err(ConstantPackRefusal::ExecutablePairMismatch);
        }
        require_abi_rows_with_lanes(
            RuntimeShaderStage::Vertex,
            &self.abi.vertex_constants,
            &seed.written_vertex,
            pass.code_constants,
        )?;
        require_abi_rows_with_lanes(
            RuntimeShaderStage::Pixel,
            &self.abi.pixel_constants,
            &seed.written_pixel,
            pass.code_constants,
        )?;
        let start = out.len();
        out.extend_from_slice(bytemuck::cast_slice(seed.vertex.as_slice()));
        out.extend_from_slice(bytemuck::cast_slice(seed.pixel.as_slice()));
        let result = pass
            .code_constants
            .iter()
            .chain(overlay)
            .try_for_each(|lane| {
                overlay_lane_bytes(lane, &mut out[start..], seed.vertex.len(), seed.pixel.len())
            });
        if result.is_err() {
            out.truncate(start);
        }
        result
    }
}
