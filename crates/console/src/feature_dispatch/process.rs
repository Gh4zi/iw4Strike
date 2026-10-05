use std::io::Write;

use bevy::prelude::*;
use net::{MasterBridge, MasterBridgeState};

const LEAVE_BUDGET: std::time::Duration = std::time::Duration::from_millis(250);

pub(crate) fn exit_process(
    mut exit: MessageReader<AppExit>,
    bridge: Option<Res<MasterBridge>>,
    settings: Option<ResMut<crate::user_settings::UserSettingsPersistence>>,
    account: Option<ResMut<crate::local_account::AccountPersistence>>,
) {
    let Some(code) = exit.read().last().map(|exit| match exit {
        AppExit::Success => 0,
        AppExit::Error(code) => i32::from(code.get()),
    }) else {
        return;
    };
    if let Some(bridge) = bridge {
        leave_master(&bridge);
    }
    if let Some(mut settings) = settings {
        settings.finish_pending_save();
    }
    if let Some(mut account) = account {
        account.finish_pending_save();
    }
    diag::lifecycle_boundary("process_exit", &format!(" code={code}"));
    diag::flush();
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}

fn leave_master(bridge: &MasterBridge) {
    if !matches!(
        bridge.state(),
        MasterBridgeState::Hosting { .. }
            | MasterBridgeState::Joining { .. }
            | MasterBridgeState::Joined { .. }
    ) {
        return;
    }
    bridge.leave();
    let until = std::time::Instant::now() + LEAVE_BUDGET;
    while std::time::Instant::now() < until {
        if bridge.state().is_terminal() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    diag::warn!(
        Console,
        "quit: master had not confirmed the leave after {}ms",
        LEAVE_BUDGET.as_millis()
    );
}
