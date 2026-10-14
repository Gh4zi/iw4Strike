//! Chat (`say`): a player's line to everyone on the server, both teams, as CS's all chat. It goes
//! out as a game print marked chat: the CS HUD's chat box shows it, MW2's HUD its game messages.

use crate::frame::FrameWorld;
use crate::{ClientId, ClientLifecycle, HUD_PRINT_ARG_SEPARATOR, HUD_STRING_PLAIN, PendingPrint};

/// Sends `text` from `id` to everyone: the print's arguments are the name (`*DEAD*` before it
/// when the sender is dead, as in CS), the line, and the sender's team for the name's colour.
pub(crate) fn say(world: &mut FrameWorld, id: ClientId, text: &str) {
    let text: String = text.chars().filter(|c| !c.is_control()).collect();
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let Some(meta) = world.client_meta(id) else {
        return;
    };
    let name = entity_iw4::client_state_name(&meta.name).unwrap_or("Player");
    let name = if meta.lifecycle == ClientLifecycle::Alive {
        name.to_owned()
    } else {
        format!("*DEAD* {name}")
    };
    let team = meta.client_state_team;
    diag::info!(Sim, "say: client {} {name}: {text}", id.0);
    world.push_print(PendingPrint {
        recipient: None,
        bold: false,
        chat: true,
        template: format!("{HUD_STRING_PLAIN}&&1^7: &&2"),
        arg: format!("{name}{HUD_PRINT_ARG_SEPARATOR}{text}{HUD_PRINT_ARG_SEPARATOR}{team}"),
    });
}
