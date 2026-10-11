use bevy::ecs::change_detection::Mut;
use bevy::prelude::Node;
use bevy::ui::{Display, Val};

/// A node a HUD system writes: read through `Deref`, written only on a real change. Writing a
/// `Mut<Node>` through `&mut Node` marks it changed even when the value is the same, and a
/// changed node sends the whole UI tree back through layout.
pub trait NodeWrite {
    fn node(&self) -> &Node;
    fn node_mut(&mut self) -> &mut Node;
}

impl NodeWrite for Node {
    fn node(&self) -> &Node {
        self
    }
    fn node_mut(&mut self) -> &mut Node {
        self
    }
}

impl NodeWrite for &mut Node {
    fn node(&self) -> &Node {
        self
    }
    fn node_mut(&mut self) -> &mut Node {
        self
    }
}

impl NodeWrite for Mut<'_, Node> {
    fn node(&self) -> &Node {
        self
    }
    fn node_mut(&mut self) -> &mut Node {
        self
    }
}

pub(crate) fn adopt_display(node: &mut (impl NodeWrite + ?Sized), want: Display) {
    if node.node().display != want {
        node.node_mut().display = want;
    }
}

pub(crate) fn adopt_box(
    node: &mut (impl NodeWrite + ?Sized),
    left: Val,
    top: Val,
    width: Val,
    height: Val,
) {
    let have = node.node();
    if have.left != left || have.top != top || have.width != width || have.height != height {
        let node = node.node_mut();
        node.left = left;
        node.top = top;
        node.width = width;
        node.height = height;
    }
}
