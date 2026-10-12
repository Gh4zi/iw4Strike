//! A Panorama-style UI runtime. CS:GO's 2018-2023 interface is Panorama: XML layouts, CSS-like
//! styles and SVG/PNG art. This crate reads those files from the player's own CS:GO install at
//! runtime (none are shipped) and builds Bevy UI from them, so a HUD or menu looks as CS:GO
//! draws it; the game feeds it what CS:GO's C++ panels set (classes, dialog variables, text,
//! images, progress).

pub mod css;
mod doc;
mod files;
pub mod style;
pub mod xml;

pub use doc::Doc;
pub use files::{Files, Fonts, decode_vfont};
