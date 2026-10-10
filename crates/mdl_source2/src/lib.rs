//! Source 2 assets as Counter-Strike 2 ships them, read at runtime from the user's own install
//! (`game/csgo/pak01_dir.vpk`). Nothing is copied or shipped.
//!
//! Every compiled Source 2 file is a resource: a header and a list of typed blocks
//! ([`resource`]). Most blocks hold binary KeyValues3 ([`kv3`]); a model's mesh buffers are
//! compressed with meshoptimizer ([`meshopt`]).
//!
//! The formats follow ValveResourceFormat (MIT, <https://github.com/ValveResourceFormat>) and
//! meshoptimizer (MIT, <https://github.com/zeux/meshoptimizer>), written anew in Rust.

pub mod anim;
pub mod kv3;
pub mod material;
pub mod meshopt;
pub mod model;
pub mod pose;
pub mod resource;
pub mod texture;
pub mod viewmodel;

pub use kv3::Value;
pub use resource::Resource;
