//! `items_game_dump <items_game.txt> <weapon_class>...`: each CS:GO weapon's attributes as the game
//! resolves them, its item's prefab chain merged from the base prefab down (`weapon_m4a1` …).

use mdl_source::kv::{self, Value};
use std::collections::BTreeMap;

/// The attributes of prefab `name` and every prefab it builds on, the nearest winning.
fn prefab_attributes(prefabs: &Value, name: &str, out: &mut BTreeMap<String, String>) {
    let Some(prefab) = prefabs.block(name) else {
        return;
    };
    if let Some(parents) = prefab.text("prefab") {
        // A prefab may build on several, space-separated; earlier ones lose to later ones.
        for parent in parents.split_whitespace() {
            prefab_attributes(prefabs, parent, out);
        }
    }
    if let Some(attributes) = prefab.block("attributes") {
        for (key, value) in attributes.entries() {
            if let Value::Text(text) = value {
                out.insert(key.clone(), text.to_owned());
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("usage: items_game_dump <items_game.txt> <weapon_class>...");
        std::process::exit(2);
    };
    let text = std::fs::read_to_string(path).expect("read items_game.txt");
    let root = kv::parse(&text);
    let game = root.block("items_game").expect("items_game block");
    let prefabs = game.block("prefabs").expect("prefabs");
    let items = game.block("items").expect("items");
    for wanted in &args[2..] {
        let Some((_, item)) = items
            .entries()
            .find(|(_, item)| item.text("name") == Some(wanted.as_str()))
        else {
            println!("{wanted}: not an item");
            continue;
        };
        let mut attributes = BTreeMap::new();
        if let Some(prefab) = item.text("prefab") {
            for name in prefab.split_whitespace() {
                prefab_attributes(prefabs, name, &mut attributes);
            }
        }
        println!("== {wanted}");
        for (key, value) in attributes {
            println!("  {key} = {value}");
        }
    }
}
