pub mod args;
pub mod bench;
mod frame_owner;
mod game_folders;
mod launch;
mod plugins;
#[cfg(all(feature = "bevy-profile", windows, target_arch = "x86_64"))]
mod stall_stacks;
#[cfg(feature = "bevy-profile")]
mod system_profile;

pub use args::{AcceptanceLaunch, LaunchMode, parse_cli};
pub use launch::launch;
pub use plugins::assemble_listen_app;
