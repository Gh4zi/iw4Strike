pub const INPUT_COMMAND_NAMES: [&str; 92] = [
    "",
    "+attack",
    "-attack",
    "+melee",
    "-melee",
    "+frag",
    "-frag",
    "+smoke",
    "-smoke",
    "+breath_sprint",
    "-breath_sprint",
    "+usereload",
    "-usereload",
    "+speed_throw",
    "-speed_throw",
    "+actionslot 1",
    "-actionslot 1",
    "+actionslot 2",
    "-actionslot 2",
    "+actionslot 3",
    "-actionslot 3",
    "+actionslot 4",
    "-actionslot 4",
    "+stance",
    "-stance",
    "+gostand",
    "-gostand",
    "+forward",
    "-forward",
    "+back",
    "-back",
    "+moveleft",
    "-moveleft",
    "+moveright",
    "-moveright",
    "+movedown",
    "-movedown",
    "+left",
    "-left",
    "+right",
    "-right",
    "+lookup",
    "-lookup",
    "+lookdown",
    "-lookdown",
    "+strafe",
    "-strafe",
    "+holdbreath",
    "-holdbreath",
    "+activate",
    "-activate",
    "+reload",
    "-reload",
    "+prone",
    "-prone",
    "+mlook",
    "-mlook",
    "+toggleads_throw",
    "-toggleads_throw",
    "+sprint",
    "-sprint",
    "+scores",
    "-scores",
    "+talk",
    "-talk",
    "togglemenu",
    "weapnext",
    "pause",
    "chatmodepublic",
    "chatmodeteam",
    "weapprev",
    "centerview",
    "togglecrouch",
    "toggleprone",
    "goprone",
    "gocrouch",
    "toggleads",
    "leaveads",
    // Counter-Strike weapon slots: 1 primary, 2 pistol, 3 knife, 4 grenades, 5 C4.
    "slot1",
    "slot2",
    "slot3",
    "slot4",
    "slot5",
    // Counter-Strike: throw the held gun.
    "drop",
    // Counter-Strike: open or close the buy menu.
    "buymenu",
    // Counter-Strike: the rest of the number keys (menu items 6-9, 0 = slot10 closes a menu).
    "slot6",
    "slot7",
    "slot8",
    "slot9",
    "slot10",
    // Counter-Strike: buy the best rifle and armor you can afford / what you bought last time.
    "autobuy",
    "rebuy",
];

/// Other games' names for the same commands: `+jump` is `+gostand`.
const COMMAND_ALIASES: [(&str, &str); 2] = [("+jump", "+gostand"), ("-jump", "-gostand")];

pub const HOLD_PAIR_LIMIT: u32 = 0x41;

pub const SCRIPT_KEYNUM: i32 = 0x400;

pub fn command_id_from_name(name: &str) -> Option<u32> {
    INPUT_COMMAND_NAMES
        .iter()
        .position(|&s| s == name)
        .map(|i| i as u32)
        .filter(|&id| id != 0)
}

pub fn command_id_lookup(name: &str) -> Option<u32> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut buf = [0u8; 32];
    let n = trimmed.len().min(buf.len());
    for (i, b) in trimmed.as_bytes()[..n].iter().enumerate() {
        buf[i] = b.to_ascii_lowercase();
    }
    let folded = core::str::from_utf8(&buf[..n]).ok()?;
    if let Some(id) = id_or_alias(folded) {
        return Some(id);
    }
    if folded.starts_with('+') || folded.starts_with('-') {
        return None;
    }
    if n + 1 > buf.len() {
        return None;
    }
    buf.copy_within(..n, 1);
    buf[0] = b'+';
    let plus = core::str::from_utf8(&buf[..n + 1]).ok()?;
    id_or_alias(plus)
}

fn id_or_alias(name: &str) -> Option<u32> {
    let name = COMMAND_ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name, |(_, command)| *command);
    command_id_from_name(name)
}

pub fn command_name(id: u32) -> Option<&'static str> {
    INPUT_COMMAND_NAMES
        .get(id as usize)
        .copied()
        .filter(|s| !s.is_empty())
}

pub fn key_up_command_id(binding: u32) -> Option<u32> {
    if binding == 0 {
        None
    } else if binding < HOLD_PAIR_LIMIT && binding % 2 == 1 {
        Some(binding + 1)
    } else {
        None
    }
}

pub fn command_names() -> impl Iterator<Item = &'static str> {
    INPUT_COMMAND_NAMES
        .iter()
        .copied()
        .filter(|s| !s.is_empty())
}
