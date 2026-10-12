use bevy::prelude::*;

use crate::ModelLightingRequest;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XModelSurfaceDraw {
    pub surface: u32,
    pub material: u32,
    pub world_from_local: Mat4,
    pub lighting_handle: u32,

    pub pending_lighting: Option<ModelLightingRequest>,

    pub colour_refusal: Option<XModelColourRefusal>,

    pub object_id: u16,

    pub scene_light_index: u8,

    pub reflection_probe_index: u8,

    pub packed_lighting: Option<[u8; 4]>,

    pub is_scope: bool,

    pub scene_entnum: Option<u32>,
    pub body_client: Option<u32>,

    /// World-space bounding sphere of the object this surface belongs to.
    /// `None` is "unbounded": the surface is admitted to every shadow
    /// partition, which is what a producer that cannot state a bound gets.
    pub caster_bound: Option<XModelCasterBound>,
}

/// A shadow caster's world-space bounding sphere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XModelCasterBound {
    pub origin: [f32; 3],
    pub radius: f32,
}

impl XModelCasterBound {
    /// `true` when the whole sphere is behind at least one plane, so no part of
    /// it is inside the volume the planes bound. Planes face inwards and carry
    /// their distance in `[3]`, as `dpvs_iw4::bounds_culled` reads them.
    #[must_use]
    pub fn outside(self, planes: &[[f32; 4]]) -> bool {
        planes.iter().any(|plane| {
            plane[0] * self.origin[0]
                + plane[1] * self.origin[1]
                + plane[2] * self.origin[2]
                + plane[3]
                + self.radius
                <= 0.0
        })
    }
}

/// The volumes each sun partition collects its casters from this frame, published before the
/// models are posed (empty while sun shadows are off). A model the camera does not see still
/// shades what it does see while its bound is inside one of them, as IW4 culls scene entities
/// for each sun partition apart from the camera.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct SunShadowCasterVolume {
    pub partitions: Vec<Vec<[f32; 4]>>,
}

impl SunShadowCasterVolume {
    #[must_use]
    pub fn admits(&self, bound: XModelCasterBound) -> bool {
        self.partitions.iter().any(|planes| !bound.outside(planes))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XModelColourRefusal {
    CameraFrustum,
    /// Another model is drawn in its place (a CS:GO body for a MW2 one): it only casts its
    /// shadow.
    StandIn,
}
