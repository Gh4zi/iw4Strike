use bevy::prelude::*;

#[derive(Resource, Default, Debug)]
pub struct UiPartyState {
    pub active: bool,
    pub in_lobby: bool,
    pub is_host: bool,
}

#[derive(Resource, Default, Debug)]
pub struct UiMenuDvars {
    values: bevy::platform::collections::HashMap<String, String>,
}

/// Menu systems republish their dvars every frame, so the common case — a name that is already
/// lower case and a value that has not changed — must not allocate.
fn lower_name(name: &str) -> std::borrow::Cow<'_, str> {
    if name.bytes().any(|b| b.is_ascii_uppercase()) {
        std::borrow::Cow::Owned(name.to_ascii_lowercase())
    } else {
        std::borrow::Cow::Borrowed(name)
    }
}

impl UiMenuDvars {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .get(lower_name(name).as_ref())
            .map(String::as_str)
    }

    pub fn set(&mut self, name: &str, value: impl AsRef<str> + Into<String>) {
        let name = lower_name(name);
        if let Some(current) = self.values.get_mut(name.as_ref()) {
            if current.as_str() != value.as_ref() {
                *current = value.into();
            }
            return;
        }
        self.values.insert(name.into_owned(), value.into());
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }
}

#[derive(Resource, Clone, Default, Debug)]
pub struct HostMatchRules(pub Vec<(String, String)>);

#[derive(Message, Clone, Debug)]
pub struct UiExecCommand {
    pub text: String,
}

#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub enum UiMenuRequest {
    Toggle,
    Open(String),
    Close(String),
    Focus { menu: String, item: String },
    Key(UiMenuKey),
    Text(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiMenuKey {
    Escape,
    Enter,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Backspace,
    Delete,
}
