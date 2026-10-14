//! CS:GO's scoreboard (TAB), built from its own layout (`layout/scoreboard.xml`) the way its
//! `scoreboard.js` builds the competitive board: the header (mode, map, round clock), the
//! Counter-Terrorists and the Terrorists with a row per player (status, ping, name, money for
//! teammates, kills, assists, deaths, MVPs, score), each side's score and players alive, and
//! between them the round timeline: each half's rounds with the winner's icon for how it was won
//! (the server's `cs_round_history`) and the halves' scores. Free-for-all shows CS:GO's
//! deathmatch board instead: one table of everyone, without teams or the timeline.

use std::collections::HashMap;

use panorama::{Doc, Files};

/// The competitive board and its player row (`_Initialize`, `_GetPlayerRowForGameMode`).
const BOARD: &str = "snippet_scoreboard-classic--with-timeline--half-times";
const ROW: &str = "snippet_scoreboard-classic__row--comp";
/// Free-for-all deathmatch's board and row (`mp_teammates_are_enemies`).
const FFA_BOARD: &str = "snippet_scoreboard--no-teams";
const FFA_ROW: &str = "snippet_scoreboard__row--deathmatch";
/// The stat set shown (`_m_dataSetCurrent`); the other is CS:GO's second page of stats.
const SHOWN_SET: &str = "0";

/// The two sides, as the board names them (`CT`, `TERRORIST`), and free-for-all's one table
/// (`ANY`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Ct,
    T,
    Any,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Self::Ct => "CT",
            Self::T => "TERRORIST",
            Self::Any => "ANY",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Ct => 0,
            Self::T => 1,
            Self::Any => 2,
        }
    }
}

/// The player row of the board for team play (`teams`) or free-for-all.
fn row_snippet(teams: bool) -> &'static str {
    if teams { ROW } else { FFA_ROW }
}

/// A player's line on the board.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Line {
    pub client: u32,
    pub side: Side,
    pub name: String,
    pub dead: bool,
    pub ping: i32,
    pub kills: i32,
    pub assists: i32,
    pub deaths: i32,
    pub score: i32,
    pub mvps: u32,
    /// Shown for teammates only (CS:GO hides the other side's money).
    pub money: Option<i32>,
    /// `dictPlayerStatusImage`: 0 none, 1 dead, 2 carrying the C4, 7 a defuse kit.
    pub status: u8,
    pub local: bool,
}

/// What the board shows.
pub(crate) struct Data {
    /// Team play; free-for-all without.
    pub teams: bool,
    pub lines: Vec<Line>,
    /// Rounds won: Counter-Terrorists, Terrorists.
    pub scores: [i32; 2],
    pub history: String,
    pub max_rounds: u32,
    pub half: u32,
    pub map: String,
    pub clock: String,
}

struct Row {
    panel: usize,
    side: Side,
    cells: HashMap<String, usize>,
}

struct Round {
    top: usize,
    bot: usize,
    tick: usize,
}

#[derive(Default)]
pub(crate) struct Scoreboard {
    /// Built, for team play (`true`) or free-for-all.
    built: Option<bool>,
    rows: HashMap<u32, Row>,
    /// Each table's rows top to bottom, as last laid out (by [`Side::index`]).
    order: [Vec<u32>; 3],
    rounds: Vec<Round>,
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

impl Scoreboard {
    /// Shows the board (`show`) with `data`, building it the first time.
    /// Hides the board (its data is only gathered while it shows).
    pub(crate) fn hide(&self, doc: &mut Doc) {
        if let Some(board) = doc.find("Scoreboard") {
            doc.set_visible(board, false);
        }
    }

    pub(crate) fn update(&mut self, doc: &mut Doc, files: &Files, show: bool, data: &Data) {
        let Some(board) = doc.find("Scoreboard") else {
            return;
        };
        doc.set_visible(board, show);
        if !show {
            return;
        }
        if self.built != Some(data.teams) {
            // The match went from team play to free-for-all or back: built again.
            if self.built.is_some() {
                for child in doc.children(board) {
                    doc.remove(child);
                }
                *self = Self::default();
            }
            self.build(doc, files, board, data);
            self.built = Some(data.teams);
        }
        self.fill_header(doc, files, board, data);
        self.fill_rows(doc, files, board, data);
        self.fill_timeline(doc, board, data);
    }

    fn build(&mut self, doc: &mut Doc, files: &Files, board: usize, data: &Data) {
        doc.load_snippet(files, board, if data.teams { BOARD } else { FFA_BOARD });
        // What this board has no use for: music kits, the meta buttons, spectators, the
        // caster buttons and website, the stats page switch.
        for id in [
            "id-sb-meta__musickit",
            "id-sb-meta__button-panel",
            "id-sb-team--Spectator",
            "spec-button-group",
            "id-sb-footer-server-website",
            "id-sb-mouse-instructions",
            "id-sb-meta__cycle",
            "id-sb-timeline__round-loss-bonus-money",
        ] {
            if let Some(panel) = doc.find_in(board, id) {
                doc.set_visible(panel, false);
            }
        }
        // The column labels (`_CreateLabelsForRow`), the second stat set kept hidden.
        if let Some(labels) = doc.find_in(board, "id-sb-players-table__labels-row") {
            let mut sets: HashMap<String, usize> = HashMap::new();
            let mut container = None;
            for (snippet, attrs) in doc.snippet_frames(row_snippet(data.teams)) {
                let Some(stat) = doc.snippet_attr(&snippet, "data-stat") else {
                    continue;
                };
                let parent = match attr(&attrs, "data-set") {
                    None => labels,
                    Some(set) => {
                        let sets_panel = *container.get_or_insert_with(|| {
                            let holder = doc
                                .add_snippet(files, labels, "snippet_sb-label-set-container")
                                .unwrap_or(labels);
                            doc.find_in(holder, "id-sb-row__sets").unwrap_or(holder)
                        });
                        *sets.entry(set.to_owned()).or_insert_with(|| {
                            let hidden = if set == SHOWN_SET { "" } else { " hidden" };
                            doc.add_element(
                                files,
                                sets_panel,
                                "Panel",
                                &[
                                    ("id", &format!("id-sb-labels-set-{set}")),
                                    ("class", &format!("sb-row__set no-hover{hidden}")),
                                ],
                            )
                        })
                    }
                };
                let id = format!("id-sb-{stat}");
                let cell = doc.add_element(
                    files,
                    parent,
                    "Button",
                    &[
                        ("id", &id),
                        ("class", &format!("sb-row__cell sb-row__cell--{stat} sb-row__cell--label")),
                    ],
                );
                let label_id = format!("label-{id}");
                if stat == "ping" {
                    doc.add_element(
                        files,
                        cell,
                        "Image",
                        &[("id", &label_id), ("src", "file://{images}/icons/ui/ping_4.svg")],
                    );
                } else {
                    let text = if attr(&attrs, "data-hidden") == Some("1") {
                        String::new()
                    } else {
                        format!("#Scoreboard_{stat}")
                    };
                    doc.add_element(files, cell, "Label", &[("id", &label_id), ("text", &text)]);
                }
            }
        }
        // The timeline (`_ResetTimeline`): the halves' rounds, a divider between them.
        if let Some(segments) = doc.find_in(board, "id-sb-timeline__segments") {
            doc.set_class(segments, "sb-team-tint", true);
            let half = data.half.max(1);
            let halves = [(1, half, "first-half"), (half + 1, data.max_rounds, "second-half")];
            for (k, &(first, last, phase)) in halves.iter().enumerate() {
                if k == 1 {
                    doc.add_element(
                        files,
                        segments,
                        "Panel",
                        &[("id", "id-sb-timeline__divider"), ("class", "sb-timeline__divider")],
                    );
                }
                let segment = doc.add_element(
                    files,
                    segments,
                    "Panel",
                    &[("id", &format!("id-sb-timeline__segment--{phase}"))],
                );
                doc.load_snippet(files, segment, "snippet_scoreboard-classic__timeline__segment");
                let Some(container) = doc.find_in(segment, "id-sb-timeline__round-container") else {
                    continue;
                };
                for rnd in first..=last {
                    let round = doc.add_element(files, container, "Panel", &[("id", &rnd.to_string())]);
                    doc.load_snippet(
                        files,
                        round,
                        "snippet_scoreboard-classic__timeline__segment__round",
                    );
                    let top = doc.find_in(round, "id-sb-timeline__segment__round--top");
                    let bot = doc.find_in(round, "id-sb-timeline__segment__round--bot");
                    let tick = doc.find_in(round, "id-sb-timeline__segment__round__tick");
                    let (Some(top), Some(bot), Some(tick)) = (top, bot, tick) else {
                        continue;
                    };
                    for data_panel in [top, bot] {
                        doc.load_snippet(
                            files,
                            data_panel,
                            "snippet_scoreboard-classic__timeline__segment__round__data",
                        );
                    }
                    if rnd % 5 == 0
                        && let Some(label) =
                            doc.find_in(round, "id-sb-timeline__segment__round__tick__label")
                    {
                        doc.set_text(label, &rnd.to_string());
                    }
                    self.rounds.push(Round { top, bot, tick });
                }
            }
        }
    }

    /// The header and the sides' names, scores and players alive (`_UpdateMatchInfo`,
    /// `_UpdateTeamInfo`, `_UpdateTeams`).
    fn fill_header(&self, doc: &mut Doc, files: &Files, board: usize, data: &Data) {
        let (icon, mode, fallback) = if data.teams {
            ("competitive", "SFUI_GameModeCompetitive", "Competitive")
        } else {
            ("deathmatch", "SFUI_GameModeDeathmatch", "Deathmatch")
        };
        if let Some(image) = doc.find_in(board, "id-sb-meta__mode__image") {
            doc.set_image(image, Some(&format!("file://{{images}}/icons/ui/{icon}.svg")));
        }
        if let Some(label) = doc.find_in(board, "id-sb-meta__labels__mode-map") {
            let mode = files.token(mode).unwrap_or(fallback);
            doc.set_text(label, &format!("{mode} | {}", data.map));
        }
        doc.set_var(board, "s_gametime_time", &data.clock);
        for (k, side) in [Side::Ct, Side::T].into_iter().enumerate() {
            let name = side.name();
            let label = if side == Side::Ct { "SFUI_CT_Label" } else { "SFUI_T_Label" };
            doc.set_var(board, &format!("sb_team_name--{name}"), files.token(label).unwrap_or(""));
            doc.set_var(board, &format!("sb_team_score--{name}"), &data.scores[k].to_string());
            let on_side = data.lines.iter().filter(|line| line.side == side);
            let total = on_side.clone().count();
            let alive = on_side.filter(|line| !line.dead).count();
            doc.set_var(board, &format!("{name}_alive"), &alive.to_string());
            doc.set_var(board, &format!("{name}_total"), &total.to_string());
        }
    }

    /// The players' rows (`_UpdatePlayer`, `_SortPlayer`): each side by score, then kills.
    fn fill_rows(&mut self, doc: &mut Doc, files: &Files, board: usize, data: &Data) {
        let sides: &[Side] = if data.teams { &[Side::Ct, Side::T] } else { &[Side::Any] };
        for &side in sides {
            let k = side.index();
            let mut lines: Vec<&Line> = data.lines.iter().filter(|line| line.side == side).collect();
            lines.sort_by(|a, b| b.score.cmp(&a.score).then(b.kills.cmp(&a.kills)));
            let order: Vec<u32> = lines.iter().map(|line| line.client).collect();
            if order != self.order[k] {
                // Panorama moves rows; this one lays the side out again.
                for client in &self.order[k] {
                    if let Some(row) = self.rows.remove(client) {
                        doc.remove(row.panel);
                    }
                }
                if let Some(table) = doc.find_in(board, &format!("players-table-{}", side.name())) {
                    for &client in &order {
                        let row = new_row(doc, files, table, side, row_snippet(data.teams));
                        self.rows.insert(client, row);
                    }
                }
                self.order[k] = order;
            }
            for line in lines {
                if let Some(row) = self.rows.get(&line.client) {
                    fill_row(doc, row, line);
                }
            }
        }
        // Rows of players gone (or moved side) go.
        let present: Vec<u32> = data.lines.iter().map(|line| line.client).collect();
        let stale: Vec<u32> = self
            .rows
            .iter()
            .filter(|(client, row)| {
                !present.contains(client)
                    || data.lines.iter().any(|l| l.client == **client && l.side != row.side)
            })
            .map(|(client, _)| *client)
            .collect();
        for client in stale {
            if let Some(row) = self.rows.remove(&client) {
                doc.remove(row.panel);
            }
            for order in &mut self.order {
                order.retain(|c| *c != client);
            }
        }
    }

    /// Each played round's winner and how (`_UpdateRound`), the next round lit, and the halves'
    /// scores. The Counter-Terrorists' row is on top; a round from before the sides switched
    /// shows on the row of the side its winners play now.
    fn fill_timeline(&self, doc: &mut Doc, board: usize, data: &Data) {
        let history = data.history.as_bytes();
        let played = (history.len() / 2) as u32;
        let half = data.half.max(1);
        let now_second_half = played + 1 > half;
        let mut halves = [[0i32; 2]; 2];
        for (i, round) in self.rounds.iter().enumerate() {
            let rnd = i as u32 + 1;
            doc.set_class(round.tick, "hilite", rnd <= played + 1);
            let flipped = (rnd > half) != now_second_half;
            let (top, bot) = if flipped { (round.bot, round.top) } else { (round.top, round.bot) };
            doc.set_class(top, "sb-team--CT", true);
            doc.set_class(top, "sb-team--TERRORIST", false);
            doc.set_class(bot, "sb-team--TERRORIST", true);
            doc.set_class(bot, "sb-team--CT", false);
            let result = history.get(2 * i..2 * i + 2);
            for (data_panel, side) in [(top, b'C'), (bot, b'T')] {
                let image = result.filter(|r| r[0] == side).map(|r| match r[1] {
                    b'b' => "file://{images}/icons/ui/bomb.svg",
                    b'd' => "file://{images}/icons/equipment/defuser.svg",
                    b't' => "file://{images}/icons/ui/timer.svg",
                    _ => "file://{images}/icons/ui/elimination.svg",
                });
                if let Some(panel) = doc.find_in(data_panel, "result") {
                    doc.set_image(panel, image);
                    doc.set_class(panel, "sb-timeline__segment__round--active", image.is_some());
                }
            }
            for side in ["sb-team--CT", "sb-team--TERRORIST"] {
                let won = result.is_some_and(|r| (r[0] == b'C') == (side == "sb-team--CT"));
                doc.set_class(round.tick, side, won);
            }
            if let Some(r) = result {
                // Who has the top row now won it when the round's winners play top now.
                let top_won = (r[0] == b'C') != flipped;
                halves[usize::from(rnd > half)][usize::from(!top_won)] += 1;
            }
        }
        for (h, var) in ["sb_team_score_2", "sb_team_score_3"].into_iter().enumerate() {
            doc.set_var(board, &format!("{var}--CT"), &halves[h][0].to_string());
            doc.set_var(board, &format!("{var}--TERRORIST"), &halves[h][1].to_string());
        }
    }
}

/// A player's row (`_NewPlayerPanel`): the `snippet` row's stat cells in order, those of a
/// stat set in that set's panel (only the shown set visible), every other cell darker.
fn new_row(doc: &mut Doc, files: &Files, table: usize, side: Side, snippet: &str) -> Row {
    let panel = doc.add_element(files, table, "Panel", &[("class", "sb-row")]);
    let mut cells = HashMap::new();
    let mut sets: HashMap<String, usize> = HashMap::new();
    let mut container = None;
    let mut idx = 0;
    for (snippet, attrs) in doc.snippet_frames(snippet) {
        let Some(stat) = doc.snippet_attr(&snippet, "data-stat") else {
            continue;
        };
        let parent = match attr(&attrs, "data-set") {
            None => panel,
            Some(set) => {
                let holder = *container.get_or_insert_with(|| {
                    doc.add_element(files, panel, "Panel", &[("id", "id-sb-row__set-container")])
                });
                *sets.entry(set.to_owned()).or_insert_with(|| {
                    idx = 0;
                    let hidden = if set == SHOWN_SET { "" } else { " hidden" };
                    doc.add_element(
                        files,
                        holder,
                        "Panel",
                        &[
                            ("id", &format!("id-sb-set-{set}")),
                            ("class", &format!("sb-row__set no-hover{hidden}")),
                        ],
                    )
                })
            }
        };
        let dark = if idx % 2 == 1 { " sb-row__cell--dark" } else { "" };
        idx += 1;
        let mut frame: Vec<(&str, &str)> = attrs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let class = format!("sb-row__cell sb-row__cell--{stat}{dark}");
        frame.push(("class", &class));
        let cell = doc.add_element(files, parent, "Frame", &frame);
        cells.insert(stat, cell);
    }
    // No ranks, flair, avatars or commendations here.
    for (stat, ids) in [
        ("rank", &["image"][..]),
        ("flair", &["image"][..]),
        ("avatar", &["image", "mute-image"][..]),
        ("name", &["id-sb-name__nameicons"][..]),
    ] {
        if let Some(&cell) = cells.get(stat) {
            for id in ids {
                if let Some(panel) = doc.find_in(cell, id) {
                    doc.set_visible(panel, false);
                }
            }
        }
    }
    Row { panel, side, cells }
}

/// Fills a row's cells with `line` (`_CreateStatUpdateFn`).
fn fill_row(doc: &mut Doc, row: &Row, line: &Line) {
    doc.set_class(row.panel, "sb-player-status-dead", line.dead);
    doc.set_class(row.panel, "sb-row--localplayer", line.local);
    let label = |doc: &Doc, stat: &str| row.cells.get(stat).and_then(|&cell| doc.find_in(cell, "label"));
    for (stat, value) in [
        ("ping", line.ping),
        ("kills", line.kills),
        ("assists", line.assists),
        ("deaths", line.deaths),
        ("score", line.score),
    ] {
        if let Some(panel) = label(doc, stat) {
            doc.set_text(panel, &value.to_string());
        }
    }
    if let Some(panel) = label(doc, "money") {
        doc.set_class(panel, "hidden", line.money.is_none());
        if let Some(money) = line.money {
            doc.set_var(panel, "stat_d_money", &money.to_string());
        }
    }
    if let Some(&cell) = row.cells.get("name")
        && let Some(panel) = doc.find_in(cell, "name")
    {
        doc.set_var(panel, "player_name", &line.name);
        doc.set_text(panel, "#Scoreboard_Player_Name");
    }
    if let Some(&cell) = row.cells.get("mvps") {
        for id in ["star-image", "star-count"] {
            if let Some(panel) = doc.find_in(cell, id) {
                doc.set_class(panel, "hidden", line.mvps == 0);
            }
        }
        if let Some(panel) = doc.find_in(cell, "star-count") {
            doc.set_text(panel, &line.mvps.to_string());
        }
    }
    if let Some(&cell) = row.cells.get("status")
        && let Some(panel) = doc.find_in(cell, "image")
    {
        let image = match line.status {
            1 => Some("file://{images}/icons/ui/elimination.svg"),
            2 => Some("file://{images}/icons/ui/bomb_c4.svg"),
            7 => Some("file://{images}/icons/equipment/defuser.svg"),
            _ => None,
        };
        doc.set_image(panel, image);
        doc.set_visible(panel, image.is_some());
    }
}
