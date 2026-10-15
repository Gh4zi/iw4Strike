//! `net_graph 1`: CS:GO's three lines over the ammo, and its rates at the right edge.
//!
//! ```text
//! fps:   308  var:  0.5 ms  ping: 0 ms                    up: 64.0/s
//! loss:   0%  choke:  0%  ver: 105                       cmd: 64.0/s
//! tick: 100.0  sv:  2.6 +- 0.5 ms   var: 0.506 ms              local
//! ```
//!
//! fps and its `var` (the frames' spread) are this game's; `ping` is a command's round trip to
//! the server's acknowledgement (0 on a listen host, as CS shows); `loss` the snapshots a gap in
//! their numbers says never came; `tick` the server's rate; `sv` its frame time and spread, and
//! the second `var` the spread of the gaps between its frames (a listen host's own server only:
//! a remote server doesn't send them). `up` counts packets sent a second, `cmd` commands.

use std::collections::VecDeque;
use std::time::Duration;

use bevy::prelude::*;
use ui::UiLayer;

/// How often the numbers change (CS:GO's move about this fast).
const REFRESH: Duration = Duration::from_millis(250);
/// The frames fps and its spread count over.
const FRAME_WINDOW: Duration = Duration::from_secs(1);
/// CS:GO's net_graph text at 1080p, and where its lines sit (from the screen's bottom).
const FONT_PX_1080: f32 = 19.0;
const LEFT_SHARE: f32 = 0.55;
const BOTTOM_PX_1080: f32 = 58.0;
const RIGHT_PX_1080: f32 = 8.0;

#[derive(Component)]
pub(crate) struct NetGraphText {
    right: bool,
}

pub(crate) fn spawn_net_graph_hud(commands: &mut Commands, font: Handle<Font>) {
    for right in [false, true] {
        commands.spawn((
            NetGraphText { right },
            UiLayer::Overlay,
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                bottom: px(BOTTOM_PX_1080),
                ..default()
            },
            GlobalZIndex(18_500),
            Text::new(""),
            TextFont {
                font: font.clone().into(),
                font_size: FontSize::Px(FONT_PX_1080),
                ..default()
            },
            TextLayout::new(
                if right { Justify::Right } else { Justify::Left },
                LineBreak::NoWrap,
            ),
            TextColor(Color::srgb(0.92, 0.92, 0.92)),
            TextShadow {
                offset: Vec2::splat(1.0),
                color: Color::srgba(0.0, 0.0, 0.0, 0.85),
            },
        ));
    }
}

/// What the overlay keeps between refreshes.
#[derive(Default)]
pub(crate) struct NetGraphState {
    frames: VecDeque<(Duration, f32)>,
    since_refresh: Duration,
    /// The link's packet counts at the last refresh, for this window's loss.
    packets: Option<(u64, u64)>,
    loss: f32,
    server: Option<((f32, f32), f32)>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_net_graph(
    settings: Res<frame::GameSettings>,
    screen: Res<frame::AppScreen>,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut stats: ResMut<net::NetGraphStats>,
    link: Option<Res<net::transport::udp_session::UdpClientLink>>,
    mut state: Local<NetGraphState>,
    mut texts: Query<(&NetGraphText, &mut Text, &mut TextFont, &mut Node, &mut Visibility)>,
) {
    let shown = settings.net_graph != 0 && *screen == frame::AppScreen::InGame;
    if !shown {
        for (.., mut visibility) in &mut texts {
            visibility.set_if_neq(Visibility::Hidden);
        }
        state.since_refresh = REFRESH;
        return;
    }
    let now = time.elapsed();
    let dt = time.delta_secs() * 1000.0;
    state.frames.push_back((now, dt));
    while state
        .frames
        .front()
        .is_some_and(|(at, _)| now.saturating_sub(*at) > FRAME_WINDOW)
    {
        state.frames.pop_front();
    }
    state.since_refresh += time.delta();
    if state.since_refresh < REFRESH {
        return;
    }
    state.since_refresh = Duration::ZERO;

    let n = state.frames.len().max(1) as f32;
    let mean = state.frames.iter().map(|(_, ms)| ms).sum::<f32>() / n;
    let spread = (state.frames.iter().map(|(_, ms)| (ms - mean).powi(2)).sum::<f32>() / n).sqrt();
    let fps = if mean > 0.0 { 1000.0 / mean } else { 0.0 };

    // A remote server's link counts its packets; a listen host's loopback loses none.
    if let Some(link) = link.as_deref() {
        let counts = (link.packets_in, link.packets_lost);
        if let Some((was_in, was_lost)) = state.packets.replace(counts) {
            let got = counts.0.saturating_sub(was_in);
            let lost = counts.1.saturating_sub(was_lost);
            if got + lost > 0 {
                state.loss = lost as f32 / (got + lost) as f32 * 100.0;
            }
        }
    } else {
        state.packets = None;
        state.loss = 0.0;
    }
    let local = link.is_none();
    let ping = if local {
        0.0
    } else {
        stats.ping_ms.unwrap_or(0.0)
    };
    if let Some(server) = net::net_graph::take_server_frames() {
        state.server = Some(server);
    } else if !local {
        state.server = None;
    }
    let (up, cmd) = stats.rates();

    let left = format!(
        "fps: {fps:>5.0}  var: {spread:>4.1} ms  ping: {ping:.0} ms\n\
         loss: {loss:>3.0}%  choke: {choke:>2}%  ver: {ver}\n\
         tick: {tick:>5.1}  sv: {sv}",
        loss = state.loss,
        choke = 0,
        ver = net::PROTOCOL_VERSION,
        tick = net::AUTHORITY_HZ,
        sv = match state.server {
            Some(((ms, dev), var)) => format!("{ms:>4.1} +- {dev:.1} ms   var: {var:.3} ms"),
            None => "-".to_owned(),
        },
    );
    let right = format!(
        "up: {up:.1}/s\ncmd: {cmd:.1}/s\n{}",
        if local { "local" } else { "remote" },
        up = up as f32,
        cmd = cmd as f32,
    );
    let scale = windows
        .single()
        .map_or(1.0, |window| window.height() / 1080.0)
        .max(0.25);
    let width = windows.single().map_or(1920.0, Window::width);
    for (which, mut text, mut font, mut node, mut visibility) in &mut texts {
        visibility.set_if_neq(Visibility::Visible);
        let want = if which.right { &right } else { &left };
        if text.0 != *want {
            text.0.clone_from(want);
        }
        let size = FontSize::Px(FONT_PX_1080 * scale);
        if font.font_size != size {
            font.font_size = size;
        }
        let (left_at, right_at) = if which.right {
            (Val::Auto, px(RIGHT_PX_1080 * scale))
        } else {
            (px(width * LEFT_SHARE), Val::Auto)
        };
        let bottom = px(BOTTOM_PX_1080 * scale);
        if node.left != left_at || node.right != right_at || node.bottom != bottom {
            node.left = left_at;
            node.right = right_at;
            node.bottom = bottom;
        }
    }
}
