//! A live Panorama document: the panel tree a layout builds (with the layouts its custom panels
//! load and the snippets it instantiates), styled by its sheets and drawn as Bevy UI nodes, and
//! the calls the game makes on it — classes, dialog variables, text, images, progress — as
//! CS:GO's C++ panels make them.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::ui::{
    BackgroundGradient, ColorStop, Gradient as UiGradient, InterpolationColorSpace, LinearGradient,
    Val2,
};

use crate::css::{self, Rule};
use crate::files::{Files, Fonts};
use crate::style::{self, Align, Flow, Len, Paint, Style};
use crate::xml::Element;

/// Labels without a size of their own: Panorama's default label text.
const DEFAULT_FONT_SIZE: f32 = 16.0;

#[derive(Clone, Debug, Default)]
struct Panel {
    tag: String,
    id: Option<String>,
    attrs: Vec<(String, String)>,
    classes: Vec<String>,
    inline: Vec<(String, String)>,
    overrides: Vec<(String, String)>,
    parent: Option<usize>,
    children: Vec<usize>,
    entity: Option<Entity>,
    text: Option<String>,
    vars: Vec<(String, String)>,
    image: Option<String>,
    /// `texturewidth`/`textureheight` (0 when not given).
    texture_size: [u32; 2],
    /// A progress bar's fill (0..1), drawn by its `ProgressBarLeft` child.
    value: f32,
    hover: bool,
    selected: bool,
    disabled: bool,
    removed: bool,
    style: Style,
    /// A texture the game gives the panel to draw (a radar it renders), over its `src`.
    image_handle: Option<Handle<Image>>,
    /// A shift (Panorama pixels) and turn (radians) the game moves the panel by every frame,
    /// applied without restyling (`set_live_transform`).
    live: Option<((f32, f32), f32, f32)>,
}

/// A loaded layout, its panels and the rules that style them.
pub struct Doc {
    panels: Vec<Panel>,
    rules: Vec<Rule>,
    /// Rules by the key their subject needs: `#id`, `.class`, `tag`, or `*`.
    index: HashMap<String, Vec<usize>>,
    styles_loaded: Vec<String>,
    snippets: HashMap<String, Element>,
    root: usize,
    /// Logical pixels per Panorama pixel.
    pub scale: f32,
    changed: bool,
    /// Panels whose live transform moved since the last sync.
    live_dirty: Vec<usize>,
}

/// The main element of a layout document: the first child of `<root>` that is not its styles,
/// scripts or snippets.
fn main_element(doc: &Element) -> Option<&Element> {
    let root = doc.child("root")?;
    root.children.iter().find(|c| {
        !["styles", "scripts", "snippets"]
            .iter()
            .any(|s| c.name.eq_ignore_ascii_case(s))
    })
}

impl css::Element for Doc {
    fn tag(&self, panel: usize) -> &str {
        &self.panels[panel].tag
    }
    fn id(&self, panel: usize) -> Option<&str> {
        self.panels[panel].id.as_deref()
    }
    fn has_class(&self, panel: usize, class: &str) -> bool {
        self.panels[panel]
            .classes
            .iter()
            .any(|c| c.eq_ignore_ascii_case(class))
    }
    fn has_pseudo(&self, panel: usize, pseudo: &str) -> bool {
        let p = &self.panels[panel];
        match pseudo {
            "hover" => p.hover,
            "selected" | "checked" => p.selected,
            "enabled" => !p.disabled,
            "disabled" => p.disabled,
            _ => false,
        }
    }
    fn parent(&self, panel: usize) -> Option<usize> {
        self.panels[panel].parent
    }
}

impl Doc {
    /// Loads `layout` (`file://{resources}/layout/hud/hud.xml`). A custom panel whose tag
    /// `sublayout` names a layout for is built from that layout, as its C++ class loads it.
    pub fn load(
        files: &Files,
        layout: &str,
        sublayout: &dyn Fn(&str) -> Option<&'static str>,
    ) -> Option<Self> {
        let mut doc = Self {
            panels: Vec::new(),
            rules: Vec::new(),
            index: HashMap::new(),
            styles_loaded: Vec::new(),
            snippets: HashMap::new(),
            root: 0,
            scale: 1.0,
            changed: true,
            live_dirty: Vec::new(),
        };
        let xml = doc.read_layout(files, layout)?;
        let main = main_element(&xml)?.clone();
        doc.root = doc.add(files, &main, None, sublayout);
        doc.reindex();
        Some(doc)
    }

    /// Reads a layout's document, taking in its styles and snippets.
    fn read_layout(&mut self, files: &Files, layout: &str) -> Option<Element> {
        let Some(text) = files.text(layout) else {
            diag::warn!(World, "panorama: no layout {layout}");
            return None;
        };
        let xml = match crate::xml::parse(&text) {
            Ok(xml) => xml,
            Err(error) => {
                diag::warn!(World, "panorama: {layout}: {error}");
                return None;
            }
        };
        if let Some(root) = xml.child("root") {
            if let Some(styles) = root.child("styles") {
                for include in &styles.children {
                    if let Some(src) = include.attr("src") {
                        self.load_styles(files, src);
                    }
                }
            }
            if let Some(snippets) = root.child("snippets") {
                for snippet in &snippets.children {
                    if let (Some(name), Some(body)) = (snippet.attr("name"), snippet.children.first())
                    {
                        self.snippets.insert(name.to_owned(), body.clone());
                    }
                }
            }
        }
        Some(xml)
    }

    fn load_styles(&mut self, files: &Files, src: &str) {
        let key = src.to_ascii_lowercase();
        if self.styles_loaded.contains(&key) {
            return;
        }
        self.styles_loaded.push(key);
        let Some(text) = files.text(src) else {
            diag::debug!(World, "panorama: no stylesheet {src}");
            return;
        };
        let parsed = css::parse(&text, self.rules.len());
        for import in &parsed.imports {
            self.load_styles(files, import);
        }
        let first = self.rules.len();
        self.rules.extend(parsed.rules);
        for (i, rule) in self.rules[first..].iter_mut().enumerate() {
            rule.order = first + i;
        }
    }

    fn reindex(&mut self) {
        self.index.clear();
        for (i, rule) in self.rules.iter().enumerate() {
            let key = rule.selector.subject().map_or_else(
                || "*".to_owned(),
                |s| {
                    if let Some(id) = &s.id {
                        format!("#{}", id.to_ascii_lowercase())
                    } else if let Some(class) = s.classes.first() {
                        format!(".{}", class.to_ascii_lowercase())
                    } else if let Some(tag) = &s.tag {
                        tag.to_ascii_lowercase()
                    } else {
                        "*".to_owned()
                    }
                },
            );
            self.index.entry(key).or_default().push(i);
        }
    }

    /// Builds `element` (and its children) under `parent`; its panel index.
    fn add(
        &mut self,
        files: &Files,
        element: &Element,
        parent: Option<usize>,
        sublayout: &dyn Fn(&str) -> Option<&'static str>,
    ) -> usize {
        let mut children: Vec<Element> = Vec::new();
        let mut classes: Vec<String> = Vec::new();
        let mut attrs = element.attrs.clone();
        if let Some(layout) = sublayout(&element.name)
            && let Some(xml) = self.read_layout(files, layout)
            && let Some(main) = main_element(&xml)
        {
            classes.extend(main.attr("class").unwrap_or("").split_whitespace().map(str::to_owned));
            for (k, v) in &main.attrs {
                if element.attr(k).is_none() {
                    attrs.push((k.clone(), v.clone()));
                }
            }
            children.extend(main.children.iter().cloned());
        }
        // A frame loads its layout only when asked for (`sublayout` names its source).
        if element.name.eq_ignore_ascii_case("Frame")
            && let Some(src) = element.attr("src")
            && sublayout(src).is_some()
            && let Some(xml) = self.read_layout(files, src)
            && let Some(main) = main_element(&xml)
        {
            children.push(main.clone());
        }
        children.extend(element.children.iter().cloned());
        let attr = |key: &str| {
            attrs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.as_str())
        };
        classes.extend(attr("class").unwrap_or("").split_whitespace().map(str::to_owned));
        let size = |key: &str| attr(key).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0).max(0) as u32;
        let panel = Panel {
            attrs: attrs.clone(),
            tag: element.name.clone(),
            id: attr("id").filter(|id| !id.is_empty()).map(str::to_owned),
            classes,
            inline: attr("style").map(css::parse_inline).unwrap_or_default(),
            parent,
            text: attr("text").map(str::to_owned),
            image: attr("src").filter(|_| !element.name.eq_ignore_ascii_case("Frame")).map(str::to_owned),
            texture_size: [size("texturewidth"), size("textureheight")],
            disabled: attr("enabled").is_some_and(|e| e == "false"),
            ..Panel::default()
        };
        let index = self.panels.len();
        self.panels.push(panel);
        if let Some(parent) = parent {
            self.panels[parent].children.push(index);
        }
        if element.name.eq_ignore_ascii_case("ProgressBar") {
            // Its fill, `<id>_Left` (`HealthBar_Left`), as Panorama names it.
            let mut attrs = vec![("class".into(), "ProgressBarLeft".into())];
            if let Some(id) = &self.panels[index].id {
                attrs.push(("id".into(), format!("{id}_Left")));
            }
            let left = Element {
                name: "Panel".into(),
                attrs,
                children: Vec::new(),
            };
            self.add(files, &left, Some(index), &|_| None);
        }
        for child in &children {
            self.add(files, child, Some(index), sublayout);
        }
        index
    }

    #[must_use]
    pub fn root(&self) -> usize {
        self.root
    }

    /// The first live panel with `id` under `scope` (itself included).
    #[must_use]
    pub fn find_in(&self, scope: usize, id: &str) -> Option<usize> {
        let p = &self.panels[scope];
        if p.removed {
            return None;
        }
        if p.id.as_deref().is_some_and(|own| own.eq_ignore_ascii_case(id)) {
            return Some(scope);
        }
        p.children.iter().find_map(|&c| self.find_in(c, id))
    }

    /// The first live panel with `id`.
    #[must_use]
    pub fn find(&self, id: &str) -> Option<usize> {
        self.find_in(self.root, id)
    }

    /// The live panels under `scope` with `class`.
    #[must_use]
    pub fn find_class_in(&self, scope: usize, class: &str) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![scope];
        while let Some(p) = stack.pop() {
            if self.panels[p].removed {
                continue;
            }
            if css::Element::has_class(self, p, class) {
                out.push(p);
            }
            stack.extend(self.panels[p].children.iter().rev());
        }
        out
    }

    #[must_use]
    pub fn children(&self, panel: usize) -> Vec<usize> {
        self.panels[panel]
            .children
            .iter()
            .copied()
            .filter(|&c| !self.panels[c].removed)
            .collect()
    }

    /// A layout attribute of `panel` (`min`, `max` of a progress bar…).
    #[must_use]
    pub fn attr(&self, panel: usize, key: &str) -> Option<&str> {
        self.panels[panel]
            .attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    #[must_use]
    pub fn entity(&self, panel: usize) -> Option<Entity> {
        self.panels[panel].entity
    }

    #[must_use]
    pub fn has_class(&self, panel: usize, class: &str) -> bool {
        css::Element::has_class(self, panel, class)
    }

    pub fn set_class(&mut self, panel: usize, class: &str, on: bool) {
        let p = &mut self.panels[panel];
        let at = p.classes.iter().position(|c| c.eq_ignore_ascii_case(class));
        match (at, on) {
            (None, true) => p.classes.push(class.to_owned()),
            (Some(i), false) => {
                p.classes.remove(i);
            }
            _ => return,
        }
        self.changed = true;
    }

    /// Sets dialog variable `name` on `panel` (its labels and their children's read it).
    pub fn set_var(&mut self, panel: usize, name: &str, value: &str) {
        let vars = &mut self.panels[panel].vars;
        match vars.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
            Some((_, v)) if v == value => return,
            Some((_, v)) => *v = value.to_owned(),
            None => vars.push((name.to_owned(), value.to_owned())),
        }
        self.changed = true;
    }

    pub fn set_text(&mut self, panel: usize, text: &str) {
        if self.panels[panel].text.as_deref() != Some(text) {
            self.panels[panel].text = Some(text.to_owned());
            self.changed = true;
        }
    }

    pub fn set_image(&mut self, panel: usize, src: Option<&str>) {
        if self.panels[panel].image.as_deref() != src {
            self.panels[panel].image = src.map(str::to_owned);
            self.changed = true;
        }
    }

    /// Draws `handle` as `panel`'s image (stretched over it), or stops (`None`).
    pub fn set_image_handle(&mut self, panel: usize, handle: Option<Handle<Image>>) {
        if self.panels[panel].image_handle != handle {
            self.panels[panel].image_handle = handle;
            self.changed = true;
        }
    }

    /// Moves `panel` by `shift` (Panorama pixels) and turns it by `turn` radians about its
    /// middle, over its styles' transform, cheaply enough to do every frame.
    pub fn set_live_transform(&mut self, panel: usize, shift: (f32, f32), turn: f32) {
        self.set_live_transform_scaled(panel, shift, turn, 1.0);
    }

    /// [`Self::set_live_transform`], also scaled by `scale` about its middle.
    pub fn set_live_transform_scaled(&mut self, panel: usize, shift: (f32, f32), turn: f32, scale: f32) {
        let live = Some((shift, turn, scale));
        if self.panels[panel].live != live {
            self.panels[panel].live = live;
            self.live_dirty.push(panel);
        }
    }

    /// A progress bar's fill, 0 to 1.
    pub fn set_value(&mut self, panel: usize, value: f32) {
        let value = value.clamp(0.0, 1.0);
        if (self.panels[panel].value - value).abs() > 1e-4 {
            self.panels[panel].value = value;
            self.changed = true;
        }
    }

    pub fn set_hover(&mut self, panel: usize, on: bool) {
        if self.panels[panel].hover != on {
            self.panels[panel].hover = on;
            self.changed = true;
        }
    }

    pub fn set_selected(&mut self, panel: usize, on: bool) {
        if self.panels[panel].selected != on {
            self.panels[panel].selected = on;
            self.changed = true;
        }
    }

    pub fn set_enabled(&mut self, panel: usize, on: bool) {
        if self.panels[panel].disabled == on {
            self.panels[panel].disabled = !on;
            self.changed = true;
        }
    }

    /// Overrides one style property on `panel` (`None` drops the override).
    pub fn set_style(&mut self, panel: usize, property: &str, value: Option<&str>) {
        let overrides = &mut self.panels[panel].overrides;
        let at = overrides.iter().position(|(k, _)| k == property);
        match (at, value) {
            (Some(i), Some(v)) if overrides[i].1 == v => return,
            (Some(i), Some(v)) => overrides[i].1 = v.to_owned(),
            (None, Some(v)) => overrides.push((property.to_owned(), v.to_owned())),
            (Some(i), None) => {
                overrides.remove(i);
            }
            (None, None) => return,
        }
        self.changed = true;
    }

    /// Shows or hides `panel` over what its styles say, as `SetVisible` does.
    pub fn set_visible(&mut self, panel: usize, on: bool) {
        self.set_style(panel, "visibility", Some(if on { "visible" } else { "collapse" }));
    }

    /// Loads snippet `name` into `panel` itself (its classes and children become the panel's),
    /// as `RequireLoadLayoutSnippet` does.
    pub fn load_snippet(&mut self, files: &Files, panel: usize, name: &str) -> bool {
        let Some(element) = self.snippets.get(name).cloned() else {
            return false;
        };
        for class in element.attr("class").unwrap_or("").split_whitespace() {
            if !css::Element::has_class(self, panel, class) {
                self.panels[panel].classes.push(class.to_owned());
            }
        }
        for child in &element.children {
            self.add(files, child, Some(panel), &|_| None);
        }
        self.changed = true;
        true
    }

    /// Builds snippet `name` as a new last child of `parent`; its panel.
    pub fn add_snippet(&mut self, files: &Files, parent: usize, name: &str) -> Option<usize> {
        let element = self.snippets.get(name)?.clone();
        let panel = self.add(files, &element, Some(parent), &|_| None);
        self.changed = true;
        Some(panel)
    }

    /// Removes `panel` and everything under it.
    pub fn remove(&mut self, panel: usize) {
        if self.panels[panel].removed {
            return;
        }
        self.panels[panel].removed = true;
        self.changed = true;
        let children = self.panels[panel].children.clone();
        for c in children {
            self.remove(c);
        }
    }

    /// A dialog variable, from `panel` up.
    fn var(&self, panel: usize, name: &str) -> Option<&str> {
        let mut at = Some(panel);
        while let Some(p) = at {
            if let Some((_, v)) = self.panels[p]
                .vars
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
            {
                return Some(v);
            }
            at = self.panels[p].parent;
        }
        None
    }

    /// A label's text: its `#token` in English, dialog variables (`{s:name}`, `{d:name}`,
    /// `{t:s:name}`) filled in, markup dropped.
    fn render_text(&self, files: &Files, panel: usize) -> String {
        let raw = self.panels[panel].text.as_deref().unwrap_or("");
        let text = raw
            .strip_prefix('#')
            .and_then(|token| files.token(token))
            .unwrap_or(raw);
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let Some(close) = rest[open..].find('}') else {
                out.push_str(&rest[open..]);
                rest = "";
                break;
            };
            let name = rest[open + 1..open + close].rsplit(':').next().unwrap_or("");
            out.push_str(self.var(panel, name).unwrap_or(""));
            rest = &rest[open + close + 1..];
        }
        out.push_str(rest);
        // Drop markup (`<b>`, `<font color=…>`).
        let mut plain = String::with_capacity(out.len());
        let mut in_tag = false;
        for c in out.chars() {
            match c {
                '<' => in_tag = true,
                '>' if in_tag => in_tag = false,
                c if !in_tag => plain.push(c),
                _ => {}
            }
        }
        plain
    }

    fn compute_style(&self, panel: usize) -> Style {
        let p = &self.panels[panel];
        let mut keys: Vec<String> = vec!["*".into(), p.tag.to_ascii_lowercase()];
        if let Some(id) = &p.id {
            keys.push(format!("#{}", id.to_ascii_lowercase()));
        }
        keys.extend(p.classes.iter().map(|c| format!(".{}", c.to_ascii_lowercase())));
        let mut matched: Vec<usize> = keys
            .iter()
            .filter_map(|k| self.index.get(k))
            .flatten()
            .copied()
            .filter(|&r| css::matches(&self.rules[r].selector, self, panel))
            .collect();
        matched.sort_unstable_by_key(|&r| (self.rules[r].specificity, self.rules[r].order));
        matched.dedup();
        let mut style = Style::default();
        for r in matched {
            for (k, v) in &self.rules[r].declarations {
                style.apply(k, v);
            }
        }
        for (k, v) in p.inline.iter().chain(&p.overrides) {
            style.apply(k, v);
        }
        if let Some(parent) = p.parent {
            style.inherit(&self.panels[parent].style);
        }
        style
    }

    /// Logical pixels per Panorama pixel (the screen's height over 1080).
    pub fn set_scale(&mut self, scale: f32) {
        if (self.scale - scale).abs() > 1e-4 {
            self.scale = scale;
            self.changed = true;
        }
    }

    /// Whether anything changed since the last [`Doc::sync`].
    #[must_use]
    pub fn changed(&self) -> bool {
        self.changed
    }

    /// Restyles the document and brings its Bevy nodes up to date, spawning panels under
    /// `parent` (the root's) as needed.
    pub fn sync(
        &mut self,
        parent: Entity,
        commands: &mut Commands,
        files: &mut Files,
        images: &mut Assets<Image>,
        fonts: &Fonts,
    ) {
        if !self.changed {
            // Only live transforms moved: set them, no restyle.
            for p in std::mem::take(&mut self.live_dirty) {
                if let (Some(entity), Some(((x, y), turn, scale))) = (self.panels[p].entity, self.panels[p].live) {
                    commands.entity(entity).insert(UiTransform {
                        translation: Val2::px(x * self.scale, y * self.scale),
                        scale: Vec2::splat(scale),
                        rotation: Rot2::radians(turn),
                    });
                }
            }
            return;
        }
        self.changed = false;
        self.live_dirty.clear();
        // Styles top-down (panels are added after their parents).
        for p in 0..self.panels.len() {
            if !self.panels[p].removed {
                self.panels[p].style = self.compute_style(p);
            }
        }
        for p in 0..self.panels.len() {
            if self.panels[p].removed {
                if let Some(entity) = self.panels[p].entity.take() {
                    commands.entity(entity).try_despawn();
                }
                continue;
            }
            let entity = match self.panels[p].entity {
                Some(e) => e,
                None => {
                    let parent_entity = self.panels[p]
                        .parent
                        .and_then(|q| self.panels[q].entity)
                        .unwrap_or(parent);
                    let e = commands.spawn((Node::default(), ChildOf(parent_entity))).id();
                    self.panels[p].entity = Some(e);
                    e
                }
            };
            self.apply(p, entity, commands, files, images, fonts);
        }
    }

    /// Effective opacity and wash of `panel` (its ancestors' multiplied in).
    fn composite(&self, panel: usize) -> (f32, Option<Color>) {
        let mut opacity = 1.0;
        let mut wash: Option<Color> = None;
        let mut at = Some(panel);
        while let Some(p) = at {
            let s = &self.panels[p].style;
            opacity *= s.opacity;
            if let Some(w) = s.wash {
                wash = Some(wash.map_or(w, |own| style::tint(own, Some(w), 1.0)));
            }
            at = self.panels[p].parent;
        }
        (opacity, wash)
    }

    fn val(&self, len: Len) -> Val {
        match len {
            Len::Fit | Len::Fill(_) | Len::OfOther(_) => Val::Auto,
            Len::Px(px) => Val::Px(px * self.scale),
            Len::Percent(p) => Val::Percent(p),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn apply(
        &self,
        panel: usize,
        entity: Entity,
        commands: &mut Commands,
        files: &mut Files,
        images: &mut Assets<Image>,
        fonts: &Fonts,
    ) {
        let p = &self.panels[panel];
        let s = &p.style;
        let parent_flow = p.parent.map_or(Flow::None, |q| self.panels[q].style.flow);
        let (opacity, wash) = self.composite(panel);
        let mut node = Node {
            display: if !s.visible {
                Display::None
            } else if s.flow == Flow::None {
                Display::Grid
            } else {
                Display::Flex
            },
            flex_direction: match s.flow {
                Flow::Down | Flow::DownWrap => FlexDirection::Column,
                _ => FlexDirection::Row,
            },
            flex_wrap: match s.flow {
                Flow::RightWrap | Flow::DownWrap => FlexWrap::Wrap,
                _ => FlexWrap::NoWrap,
            },
            flex_shrink: 0.0,
            width: self.val(s.width),
            height: self.val(s.height),
            min_width: s.min_width.map_or(Val::Auto, |l| self.val(l)),
            min_height: s.min_height.map_or(Val::Auto, |l| self.val(l)),
            max_width: s.max_width.map_or(Val::Auto, |l| self.val(l)),
            max_height: s.max_height.map_or(Val::Auto, |l| self.val(l)),
            padding: UiRect {
                left: self.val(s.padding[0]),
                top: self.val(s.padding[1]),
                right: self.val(s.padding[2]),
                bottom: self.val(s.padding[3]),
            },
            border: UiRect {
                left: Val::Px(s.border_width[0] * self.scale),
                top: Val::Px(s.border_width[1] * self.scale),
                right: Val::Px(s.border_width[2] * self.scale),
                bottom: Val::Px(s.border_width[3] * self.scale),
            },
            border_radius: BorderRadius::all(self.val(s.border_radius)),
            overflow: if s.clip {
                Overflow::clip()
            } else {
                Overflow::visible()
            },
            ..default()
        };
        if s.flow == Flow::None {
            // One cell all children share: the panel's own size when it has one (children
            // overflow it, never grow it), else as big as they are, margins and offsets counted.
            let track = |sized: bool| {
                if sized {
                    RepeatedGridTrack::minmax(
                        1,
                        MinTrackSizingFunction::Px(0.0),
                        MaxTrackSizingFunction::Fraction(1.0),
                    )
                } else {
                    RepeatedGridTrack::auto(1)
                }
            };
            node.grid_template_columns = vec![track(!matches!(s.width, Len::Fit))];
            node.grid_template_rows = vec![track(!matches!(s.height, Len::Fit))];
            node.justify_content = JustifyContent::Stretch;
            node.align_content = AlignContent::Stretch;
        }
        // Margins, with `x`/`y` as offsets from them.
        let offset = |margin: Len, by: Len| match (margin, by) {
            (Len::Px(m), Len::Px(o)) => Val::Px((m + o) * self.scale),
            (Len::Percent(m), Len::Px(o)) if o == 0.0 => Val::Percent(m),
            (Len::Px(m), Len::Percent(o)) if m == 0.0 => Val::Percent(o),
            (m, _) => self.val(m),
        };
        node.margin = UiRect {
            left: offset(s.margin[0], s.x),
            top: offset(s.margin[1], s.y),
            right: self.val(s.margin[2]),
            bottom: self.val(s.margin[3]),
        };
        let fill = |len: Len, along: bool| matches!(len, Len::Fill(_)) && along;
        let along_row = matches!(parent_flow, Flow::Right | Flow::RightWrap);
        let along_column = matches!(parent_flow, Flow::Down | Flow::DownWrap);
        if fill(s.width, along_row) || fill(s.height, along_column) {
            let weight = match (s.width, s.height) {
                (Len::Fill(w), _) if along_row => w,
                (_, Len::Fill(w)) => w,
                _ => 1.0,
            };
            node.flex_grow = weight;
            node.flex_basis = Val::Px(0.0);
        }
        if matches!(s.width, Len::Fill(_)) && !along_row {
            node.width = Val::Percent(100.0);
        }
        // A full-size panel with margins: Panorama fits it and its margins inside the parent
        // (where CSS adds the margins outside), so it fills what the margins leave.
        let margined = |a: Len, b: Len| !matches!(a, Len::Px(0.0)) || !matches!(b, Len::Px(0.0));
        if s.width == Len::Percent(100.0) && margined(s.margin[0], s.margin[2]) {
            node.width = Val::Auto;
            match parent_flow {
                Flow::None => node.justify_self = JustifySelf::Stretch,
                Flow::Down | Flow::DownWrap => node.align_self = AlignSelf::Stretch,
                Flow::Right | Flow::RightWrap => {
                    node.flex_grow = 1.0;
                    node.flex_basis = Val::Px(0.0);
                }
            }
        }
        if s.height == Len::Percent(100.0) && margined(s.margin[1], s.margin[3]) {
            node.height = Val::Auto;
            match parent_flow {
                Flow::None => node.align_self = AlignSelf::Stretch,
                Flow::Right | Flow::RightWrap => node.align_self = AlignSelf::Stretch,
                Flow::Down | Flow::DownWrap => {
                    node.flex_grow = 1.0;
                    node.flex_basis = Val::Px(0.0);
                }
            }
        }
        // One side a share of the other: an aspect ratio (width over height).
        match (s.width, s.height) {
            (Len::OfOther(share), _) => node.aspect_ratio = Some(share),
            (_, Len::OfOther(share)) if share > 0.0 => node.aspect_ratio = Some(1.0 / share),
            _ => {}
        }
        if matches!(s.height, Len::Fill(_)) && !along_column {
            node.height = Val::Percent(100.0);
        }
        let to_justify = |a: Option<Align>| match a {
            Some(Align::Center) => JustifySelf::Center,
            Some(Align::End) => JustifySelf::End,
            _ => JustifySelf::Start,
        };
        let to_align = |a: Option<Align>| match a {
            Some(Align::Center) => AlignSelf::Center,
            Some(Align::End) => AlignSelf::End,
            _ => AlignSelf::Start,
        };
        // The first flowed child aligned to the far end of its flow (or its middle) takes the
        // free space before it, which packs it and those after it there, as Panorama does.
        let parent_style = p.parent.map(|q| &self.panels[q].style);
        let first_along = |along_end: &dyn Fn(&Style) -> Option<Align>| {
            p.parent.is_some_and(|q| {
                self.panels[q]
                    .children
                    .iter()
                    .copied()
                    .filter(|&c| !self.panels[c].removed && self.panels[c].style.visible)
                    .find(|&c| matches!(along_end(&self.panels[c].style), Some(Align::End | Align::Center)))
                    == Some(panel)
            })
        };
        match parent_flow {
            Flow::None => {
                node.grid_row = GridPlacement::start(1);
                node.grid_column = GridPlacement::start(1);
                if node.justify_self != JustifySelf::Stretch {
                    node.justify_self = to_justify(s.halign);
                }
                if node.align_self != AlignSelf::Stretch {
                    node.align_self = to_align(s.valign);
                }
            }
            Flow::Right | Flow::RightWrap => {
                if node.align_self != AlignSelf::Stretch {
                    node.align_self = to_align(s.valign);
                }
                let room = parent_style.is_some_and(|ps| !matches!(ps.width, Len::Fit));
                if room && first_along(&|st: &Style| st.halign) {
                    node.margin.left = Val::Auto;
                    if s.halign == Some(Align::Center) {
                        node.margin.right = Val::Auto;
                    }
                }
            }
            Flow::Down | Flow::DownWrap => {
                if node.align_self != AlignSelf::Stretch {
                    node.align_self = to_align(s.halign);
                }
                let room = parent_style.is_some_and(|ps| !matches!(ps.height, Len::Fit));
                if room && first_along(&|st: &Style| st.valign) {
                    node.margin.top = Val::Auto;
                    if s.valign == Some(Align::Center) {
                        node.margin.bottom = Val::Auto;
                    }
                }
            }
        }

        let mut e = commands.entity(entity);
        // Background: a colour, or a gradient over nothing.
        match &s.background {
            Some(Paint::Color(c)) => {
                e.insert(BackgroundColor(style::tint(*c, None, opacity)));
                e.remove::<BackgroundGradient>();
            }
            Some(Paint::Gradient(g)) => {
                e.insert((
                    BackgroundColor(Color::NONE),
                    BackgroundGradient(vec![UiGradient::Linear(LinearGradient {
                        color_space: InterpolationColorSpace::default(),
                        angle: g.angle,
                        stops: g
                            .stops
                            .iter()
                            .map(|(t, c)| {
                                ColorStop::new(style::tint(*c, None, opacity), Val::Percent(t * 100.0))
                            })
                            .collect(),
                    })]),
                ));
            }
            None => {
                e.insert(BackgroundColor(Color::NONE));
                e.remove::<BackgroundGradient>();
            }
        }
        e.insert(BorderColor::all(
            s.border_color.map_or(Color::NONE, |c| style::tint(c, None, opacity)),
        ));
        // Bevy turns a node about its middle; a turn about `transform-origin` is that plus the
        // shift the origin would make (in shares of the node, its aspect known or square).
        let aspect = match s.width {
            Len::OfOther(share) => share,
            _ => 1.0,
        };
        let (dx, dy) = ((s.origin.0 - 0.5) * aspect, s.origin.1 - 0.5);
        let (sin, cos) = s.rotate.sin_cos();
        let (rx, ry) = (cos * dx - sin * dy, sin * dx + cos * dy);
        let shift = ((dx - rx) / aspect * 100.0, (dy - ry) * 100.0);
        let mut px = (s.translate.0 * self.scale, s.translate.1 * self.scale);
        if parent_flow == Flow::None {
            let half = |a: Len, b: Len| match (a, b) {
                (Len::Px(a), Len::Px(b)) => (a - b) * 0.5 * self.scale,
                _ => 0.0,
            };
            if s.halign == Some(Align::Center) {
                px.0 += half(s.margin[0], s.margin[2]);
            }
            if s.valign == Some(Align::Center) {
                px.1 += half(s.margin[1], s.margin[3]);
            }
        }
        let along = |px: f32, share: f32| {
            if share.abs() < 1e-4 {
                Val::Px(px)
            } else if px == 0.0 {
                Val::Percent(share)
            } else {
                Val::Px(px)
            }
        };
        if let Some(((x, y), turn, scale)) = p.live {
            e.insert(UiTransform {
                translation: Val2::px(x * self.scale, y * self.scale),
                scale: Vec2::splat(scale),
                rotation: Rot2::radians(turn),
            });
        } else {
            e.insert(UiTransform {
                translation: Val2::new(along(px.0, shift.0), along(px.1, shift.1)),
                scale: Vec2::new(s.scale.0, s.scale.1),
                rotation: Rot2::radians(s.rotate),
            });
        }
        if let Some(z) = s.z_index {
            e.insert(ZIndex(z));
        }
        // A progress bar's fill.
        if css::Element::has_class(self, panel, "ProgressBarLeft")
            && let Some(bar) = p.parent
        {
            node.width = Val::Percent(self.panels[bar].value * 100.0);
            node.height = Val::Percent(100.0);
        }

        let tag = p.tag.to_ascii_lowercase();
        if tag == "label" {
            let mut text = self.render_text(files, panel);
            if s.uppercase == Some(true) {
                text = text.to_uppercase();
            }
            let color = style::tint(s.color.unwrap_or(Color::WHITE), wash, opacity);
            let justify = match s.text_align {
                Some(Align::Center) => Justify::Center,
                Some(Align::End) => Justify::Right,
                _ => Justify::Left,
            };
            let wraps = matches!(s.width, Len::Px(_) | Len::Percent(_) | Len::Fill(_));
            e.insert((
                Text::new(text),
                TextFont {
                    font: fonts
                        .face(s.font_family.as_deref(), s.font_weight.as_deref())
                        .into(),
                    font_size: FontSize::Px(s.font_size.unwrap_or(DEFAULT_FONT_SIZE) * self.scale),
                    ..default()
                },
                TextColor(color),
                TextLayout::new(
                    justify,
                    if wraps {
                        LineBreak::WordBoundary
                    } else {
                        LineBreak::NoWrap
                    },
                ),
            ));
            if let Some((x, y, c)) = s.text_shadow {
                e.insert(TextShadow {
                    offset: Vec2::new(x, y) * self.scale,
                    color: style::tint(c, None, opacity),
                });
            }
        }
        // A texture the game draws into, else its own `src` or a panel's `background-image`.
        if let Some(handle) = p.image_handle.clone().filter(|_| s.visible) {
            let mut image = ImageNode::new(handle);
            image.color = style::tint(Color::WHITE, wash, opacity);
            image.image_mode = NodeImageMode::Stretch;
            e.insert(image);
            e.insert(node);
            return;
        }
        let src = if tag == "image" {
            p.image.clone()
        } else {
            s.background_image.clone()
        };
        match src {
            Some(src) if s.visible => {
                let want = |len: Len, tex: u32| match len {
                    Len::Px(px) => (px * self.scale).round() as u32,
                    _ if tex > 0 => (tex as f32 * self.scale).round() as u32,
                    _ => 0,
                };
                let size = [want(s.width, p.texture_size[0]), want(s.height, p.texture_size[1])];
                if let Some((handle, px)) = files.image(&src, size, images) {
                    let mut image = ImageNode::new(handle);
                    image.color = style::tint(Color::WHITE, wash, opacity);
                    if tag == "image" {
                        // Without a size of its own, the image's (drawn at the asked scale).
                        let svg_scale = if src.ends_with(".svg") { 1.0 } else { self.scale };
                        if matches!(s.width, Len::Fit) {
                            node.width = Val::Px(px[0] as f32 * svg_scale);
                        }
                        if matches!(s.height, Len::Fit) {
                            node.height = Val::Px(px[1] as f32 * svg_scale);
                        }
                    } else {
                        image.image_mode = NodeImageMode::Stretch;
                    }
                    e.insert(image);
                } else {
                    e.remove::<ImageNode>();
                }
            }
            _ => {
                e.remove::<ImageNode>();
            }
        }
        e.insert(node);
    }
}
