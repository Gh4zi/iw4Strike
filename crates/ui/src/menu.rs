use bevy::prelude::*;
use frame::ClientSet;

use crate::classes::store::{
    ClassStoreFile, load_class_store, save_class_store, sync_host_class_loadouts,
};

#[derive(Resource, Clone, Debug, Default)]
pub struct MenuMapList(pub Vec<asset_transport::MapPack>);

impl MenuMapList {
    pub fn maps(&self) -> impl Iterator<Item = &String> {
        self.0.iter().flat_map(|pack| &pack.maps)
    }

    pub fn contains(&self, map: &str) -> bool {
        self.maps().any(|installed| installed == map)
    }

    pub fn pack_of(&self, map: &str) -> Option<usize> {
        self.0
            .iter()
            .position(|pack| pack.maps.iter().any(|installed| installed == map))
    }
}

pub(crate) struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MenuMapList>()
            .init_resource::<crate::barracks::BarracksProfile>()
            .init_resource::<crate::ClassLoadoutCatalog>()
            .init_resource::<frame::GameSettings>()
            .init_resource::<crate::BindingView>()
            .init_resource::<crate::SessionClassStore>()
            .init_resource::<ClassStoreFile>()
            .init_resource::<frame::HostClassLoadouts>()
            .init_resource::<asset_game::MenuCatalog>()
            .init_resource::<asset_game::LocalizeCatalog>()
            .add_systems(
                Update,
                (
                    crate::options::apply_window_settings,
                    load_class_store,
                    crate::barracks::load_profile,
                    crate::barracks::save_profile,
                    sync_host_class_loadouts,
                    save_class_store,
                )
                    .chain()
                    .in_set(ClientSet::Ui),
            );
    }
}

pub fn install_frontend_menus(catalog: &mut asset_game::MenuCatalog) -> Result<(), String> {
    catalog.load_definitions(include_str!("../menus/frontend.json"))?;
    catalog.load_definitions(include_str!("../menus/connection_error.json"))?;
    catalog.load_definitions(include_str!("../menus/classes.json"))?;
    catalog.load_definitions(include_str!("../menus/barracks.json"))?;
    catalog.load_definitions(include_str!("../menus/settings.json"))?;
    catalog.load_definitions(include_str!("../menus/controller.json"))?;
    let slider = catalog
        .get("pc_options_video")
        .and_then(|menu| {
            menu.items
                .iter()
                .find(|item| item.name == "video_brightness")
        })
        .cloned();
    for (name, menu) in &mut catalog.menus {
        if name == "pc_options_look"
            && let Some(template) = &slider
            && let Some(y) = menu
                .items
                .iter()
                .find(|item| item.text_key == "@MENU_MOUSE_SENSITIVITY")
                .map(|label| label.rect.y)
            && let Some(item) = menu
                .items
                .iter_mut()
                .find(|item| item.item_type == asset_game::ITEM_TYPE_SLIDER)
        {
            *item = asset_game::MenuItem {
                name: "look_sensitivity".into(),
                dvar: "ui_sensitivity".into(),
                slider: Some(asset_game::MenuSlider {
                    min: 0.1,
                    max: 30.0,
                    step: 0.1,
                    display_range: None,
                    decimals: 1,
                    suffix: String::new(),
                }),
                ..template.clone()
            };
            item.rect.y = y;
        }
        if matches!(name.as_str(), "popup_endgame" | "popup_endgame_ranked") {
            for item in &mut menu.items {
                if item.name == "button_yes" {
                    item.handlers.action = vec![asset_game::MenuEvent::Script(
                        "play mouse_click; close self; exec \"disconnect\";".into(),
                    )];
                }
            }
        }
        if let Some(settings_link) = menu
            .items
            .iter()
            .find(|item| {
                item.item_type == 1
                    && matches!(item.text_key.as_str(), "@MENU_CHAT" | "@MENU_VOICE")
            })
            .cloned()
            && menu
                .items
                .iter()
                .any(|item| item.text_key == "@MENU_RESET_SYSTEM_DEFAULTS")
        {
            let mut multiplayer = settings_link;
            multiplayer.name = "multiplayer_settings".into();
            multiplayer.text_key = "@MENU_MULTIPLAYER_OPTIONS".into();
            multiplayer.rect.y = 88.0;
            multiplayer.vis_exp = "1".into();
            multiplayer.disabled_exp = "0".into();
            multiplayer.handlers.action = vec![asset_game::MenuEvent::Script(
                "play mouse_click; close self; open options_multi;".into(),
            )];
            let mut controller = multiplayer.clone();
            menu.items.push(multiplayer);
            controller.name = "controller_settings".into();
            controller.text_key = "Controller".into();
            controller.rect.y = 108.0;
            controller.handlers.action = vec![asset_game::MenuEvent::Script(
                "play mouse_click; close self; open options_controller;".into(),
            )];
            let mut folders = controller.clone();
            menu.items.push(controller);
            folders.name = "game_folders".into();
            folders.text_key = "Game Folders".into();
            folders.rect.y = 128.0;
            folders.handlers.action = vec![asset_game::MenuEvent::Script(
                "play mouse_click; exec \"game_paths\";".into(),
            )];
            menu.items.push(folders);
        }

        let removed_rows: Vec<_> = menu
            .items
            .iter()
            .filter(|item| {
                item.item_type == 1
                    && matches!(
                        item.text_key.as_str(),
                        "@MENU_VOICE" | "@MENU_CHAT" | "@MENU_RESET_SYSTEM_DEFAULTS"
                    )
            })
            .map(|item| (item.rect.x, item.rect.y))
            .collect();
        menu.items.retain(|item| {
            !removed_rows.iter().any(|&(x, y)| {
                item.rect.x == x
                    && item.rect.y == y
                    && item.name != "multiplayer_settings"
                    && item.name != "controller_settings"
                    && item.name != "game_folders"
            })
        });
        if name == "pc_options_controls" {
            for item in &mut menu.items {
                if item.rect.x >= 232.0 && item.rect.y > 88.0 {
                    item.rect.y -= 20.0;
                }
            }
        }

        let Some(mode) = name.strip_prefix("settings_quick_") else {
            continue;
        };
        if sim::HostGameModeSelection::from_token(mode).is_none() {
            continue;
        }
        add_bot_rows(&mut menu.items);
        // CS has no perks or killstreaks: their rows go, the rows below move up.
        hide_rule_rows(&mut menu.items, &["scr_game_perks", "scr_game_hardpoints"]);
        for item in &mut menu.items {
            if item.dvar == "camera_thirdperson" {
                if item.item_type == 12 {
                    item.choices.clear();
                    item.dvar.clear();
                    item.text_key = "Недоступно".into();
                }
                item.disabled_exp = "1".into();
                item.item_type = 0;
                item.static_flags |= 0x100000;
                item.handlers.action.clear();
                item.fore_color[3] = 0.4;
            } else if item.item_type == 1 && !item.dvar.is_empty() {
                item.text_scale = 0.30;
            }
        }
    }
    Ok(())
}

/// Game Rules > Team Options rows of ours, copies of a stock toggle (button plus value
/// display) under the panel's last row: how many enemy bots, and in team modes how many
/// friendly bots, join when the match starts.
fn add_bot_rows(items: &mut Vec<asset_game::MenuItem>) {
    let pair = |items: &[asset_game::MenuItem], dvar: &str| {
        let button = items
            .iter()
            .position(|item| item.item_type == 1 && item.dvar == dvar)?;
        let value = items
            .iter()
            .position(|item| item.item_type == 12 && item.dvar == dvar)?;
        Some((button, value))
    };
    let counts = |max: u32| -> Vec<(String, String)> {
        (0..=max).map(|n| (n.to_string(), n.to_string())).collect()
    };
    let Some(gameplay) = pair(items, "scr_game_onlyheadshots") else {
        return;
    };
    // Team Options: the lowest row left of Gameplay Options in its panel.
    let (left_x, top_y) = (items[gameplay.0].rect.x, items[gameplay.0].rect.y);
    let Some((button, value)) = items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.item_type == 1 && item.rect.x < left_x - 1.0 && item.rect.y >= top_y - 1.0
        })
        .filter_map(|(i, item)| pair(items, &item.dvar).filter(|(button, _)| *button == i))
        .max_by(|a, b| items[a.0].rect.y.total_cmp(&items[b.0].rect.y))
    else {
        return;
    };
    let team_based = items.iter().any(|item| item.dvar == "scr_team_fftype");
    let mut rows = vec![("enemy_bots", "Enemy Bots:", sim::ENEMY_BOTS_DVAR, MAX_RULE_ENEMY_BOTS)];
    if team_based {
        rows.push((
            "friendly_bots",
            "Friendly Bots:",
            sim::FRIENDLY_BOTS_DVAR,
            MAX_RULE_FRIENDLY_BOTS,
        ));
    }
    let step = items[button].rect.h;
    let mut y = items[button].rect.y + step;
    for (name, label, dvar, max) in rows {
        let choices = counts(max);
        let values = choices
            .iter()
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let mut row_button = items[button].clone();
        let mut row_value = items[value].clone();
        row_button.name = format!("sidenav_button_{name}");
        row_button.text_key = label.into();
        row_button.text_literal = true;
        row_button.dvar = dvar.into();
        row_button.rect.y = y;
        row_button.handlers.action = vec![asset_game::MenuEvent::Script(format!(
            "play mouse_click; exec \"toggle {dvar} {values}\";"
        ))];
        row_value.dvar = dvar.into();
        row_value.rect.y = y;
        row_value.choices = choices;
        y += step;
        items.push(row_button);
        items.push(row_value);
    }
}

/// MW2 lobbies hold 18: nine a side.
pub const MAX_RULE_ENEMY_BOTS: u32 = 9;
pub const MAX_RULE_FRIENDLY_BOTS: u32 = 8;

/// Hides the Game Rules rows of `dvars` (button and value) and moves the rows below them in
/// the same column up into the gap.
fn hide_rule_rows(items: &mut [asset_game::MenuItem], dvars: &[&str]) {
    let hidden: Vec<(f32, f32, f32)> = items
        .iter()
        .filter(|item| item.item_type == 1 && dvars.contains(&item.dvar.as_str()))
        .map(|item| (item.rect.x, item.rect.y, item.rect.h))
        .collect();
    for item in items.iter_mut() {
        if matches!(item.item_type, 1 | 12) && dvars.contains(&item.dvar.as_str()) {
            item.vis_exp = "0".into();
            item.disabled_exp = "1".into();
            continue;
        }
        if !matches!(item.item_type, 1 | 12) {
            continue;
        }
        let rise: f32 = hidden
            .iter()
            .filter(|(x, y, _)| item.rect.x >= *x - 1.0 && item.rect.y > *y + 0.5)
            .map(|(_, _, h)| *h)
            .sum();
        item.rect.y -= rise;
    }
}
