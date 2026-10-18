use alloc::vec::Vec;

use crate::ir::Sm3Register;

pub use d3d9_decl::{DeclType, Semantic};
pub use d3d9_state::{ALPHA_REF_SCALE, AlphaTest, CompareFunc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SamplerTextureDimension {
    D2,
    Cube,
    D3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VertexInput {
    pub register: Sm3Register,
    pub location: Option<u32>,
    pub decl_type: Option<DeclType>,
    pub semantic: Semantic,
}

pub fn missing_vertex_element(semantic: Semantic) -> [f32; 4] {
    match semantic.usage {
        d3d9_decl::D3DDECLUSAGE_COLOR | d3d9_decl::D3DDECLUSAGE_TEXCOORD => [0.0, 0.0, 0.0, 1.0],
        _ => [0.0, 0.0, 0.0, 0.0],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VaryingLink {
    pub semantic: Semantic,
    pub vertex_register: Sm3Register,
    pub pixel_register: Option<Sm3Register>,
    pub location: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConstantSlot {
    pub register: u16,

    pub program_defined: bool,

    /// A value shared by many draws of a frame (the view-projection, the shadow lookup). The
    /// draw's own row holds the index of the row with the value, so a draw's rows stay the same
    /// while the camera moves. Not in a stage that indexes its constants (`c[a0.x + n]`).
    pub frame_shared: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SamplerSlot {
    pub register: u16,
    pub dimension: SamplerTextureDimension,

    pub depth_compare: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassLoweringAbi {
    pub vertex_inputs: Vec<VertexInput>,
    pub position: VaryingLink,
    pub varyings: Vec<VaryingLink>,
    pub vertex_constants: Vec<ConstantSlot>,
    pub pixel_constants: Vec<ConstantSlot>,
    pub samplers: Vec<SamplerSlot>,

    pub alpha_tests: Vec<AlphaTest>,
}
