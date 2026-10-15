use bevy::prelude::Resource;
use sim::{ClientId, LifeSequence};

use crate::controller::HostController;

pub const MAX_HOST_BOTS: u32 = 20;

const MAX_CLIENT_SLOTS: u32 = playerstate_iw4::PLAYER_CORPSE_ENTITY_BASE as u32;

#[derive(Resource, Debug, Default)]
pub struct BotAddQueue(pub Vec<BotAddRequest>);

/// Bots to take off the server (`bot kick`), at the next update.
#[derive(Resource, Debug, Default)]
pub struct BotKickQueue(pub Vec<BotKick>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BotKick {
    All,
    One(ClientId),
    /// `count` bots of one side (the Terrorists or the Counter-Terrorists), the latest added
    /// first.
    Side { terrorist: bool, count: u32 },
}

#[derive(Debug)]
pub struct BotAddRequest {
    pub count: u32,
    pub dummy: bool,
    pub side: Option<BotSide>,
}

/// The team a bot joins, relative to the local player. Without one the game scripts
/// auto-assign it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BotSide {
    Friendly,
    Enemy,
}

impl BotAddQueue {
    pub fn push(&mut self, count: u32) {
        self.0.push(BotAddRequest {
            count: count.max(1),
            dummy: false,
            side: None,
        });
    }

    /// `count` bots on the local player's side or the other (none when 0).
    pub fn push_side(&mut self, count: u32, side: BotSide) {
        if count > 0 {
            self.0.push(BotAddRequest {
                count,
                dummy: false,
                side: Some(side),
            });
        }
    }

    pub fn push_dummy(&mut self, count: u32) {
        self.0.push(BotAddRequest {
            count: count.max(1),
            dummy: true,
            side: None,
        });
    }

    pub fn drain(&mut self) -> Vec<BotAddRequest> {
        core::mem::take(&mut self.0)
    }
}

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct BotHold(pub bool);

/// CS:GO's `bot_mimic`: every bot sends the copied player's last command each tick, buttons and
/// all, its view turned by `yaw_offset` (`bot_mimic_yaw_offset`, 180 by default so the bots
/// face that player). `target` 0 is off, 1 the host, N client N - 1.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct BotMimic {
    pub target: u32,
    pub yaw_offset: f32,
}

impl Default for BotMimic {
    fn default() -> Self {
        Self {
            target: 0,
            yaw_offset: 180.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BotTpTarget {
    All,
    Id(ClientId),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BotTpWhere {
    Absolute {
        origin: [f32; 3],
        yaw: Option<f32>,
        pitch: Option<f32>,
    },

    Above {
        height: f32,
    },

    /// The host onto the bot's head (the first bot for `All`).
    Ride,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BotTpRequest {
    pub target: BotTpTarget,
    pub where_: BotTpWhere,
}

#[derive(Resource, Debug, Default)]
pub struct BotTpQueue(pub Vec<BotTpRequest>);

impl BotTpQueue {
    pub fn push(&mut self, request: BotTpRequest) {
        self.0.push(request);
    }

    pub fn drain(&mut self) -> Vec<BotTpRequest> {
        core::mem::take(&mut self.0)
    }
}

/// Test controls (`bot attack`, `bot weapon`): bots that keep attack held, and the weapon a bot
/// is told to hold, by weapon id.
#[derive(Resource, Debug, Default)]
pub struct BotTestControls {
    pub attack_all: bool,
    pub attack: std::collections::BTreeSet<ClientId>,
    /// Bots that keep use (E) held.
    pub use_held: std::collections::BTreeSet<ClientId>,
    pub weapon: std::collections::BTreeMap<ClientId, u16>,
}

#[derive(Resource, Debug, Default)]
pub struct BotFireQueue(pub Vec<BotTpTarget>);

impl BotFireQueue {
    pub fn push(&mut self, target: BotTpTarget) {
        self.0.push(target);
    }

    pub fn drain(&mut self) -> Vec<BotTpTarget> {
        core::mem::take(&mut self.0)
    }
}

#[derive(Debug)]
pub struct BotSlot {
    pub id: ClientId,
    pub account: sim::AccountId,
    pub brain: Option<HostController>,
    pub joined: bool,
    /// When it last asked to join (its team and class).
    pub joined_at: Option<std::time::Instant>,
    pub class_picked_in: Option<LifeSequence>,
    pub class_picks: u32,
    /// The command the brain last produced, resent on the ticks between thinks.
    pub last_cmd: Option<playerstate_iw4::UserCmd>,
    /// The side it joins relative to the local player (Game Rules bots), or auto-assigned.
    pub side: Option<BotSide>,
}

#[derive(Resource, Debug)]
pub struct BotRoster {
    pub bots: Vec<BotSlot>,
    /// The Game Rules' enemy and friendly bots were queued for this match.
    pub rules_filled: bool,
    pub next_client: u32,
    pub seed: u64,
}

impl Default for BotRoster {
    fn default() -> Self {
        Self {
            bots: Vec::new(),
            rules_filled: false,

            next_client: 1,
            seed: 0xb075_0001,
        }
    }
}

impl BotRoster {
    pub fn is_bot(&self, id: ClientId) -> bool {
        self.bots.iter().any(|bot| bot.id == id)
    }

    // `taken` are the ids real clients already own. A bot minted onto one of
    // them *is* that client as far as the roster is concerned: `is_bot` claims
    // the player, and the slot is dead weight because no system can drive an
    // id someone else is already playing.
    pub fn add_bots(
        &mut self,
        count: u32,
        taken: &[ClientId],
        dummy: bool,
        side: Option<BotSide>,
    ) -> Vec<ClientId> {
        let room = MAX_HOST_BOTS.saturating_sub(self.bots.len() as u32);
        let count = count.min(room);
        let seed = self.seed;
        let mut added = Vec::with_capacity(count as usize);
        for i in 0..count {
            let Some(id) = self.claim_id(taken) else {
                break;
            };
            let mut account = [0; 16];
            while account == [0; 16] {
                if let Err(error) = getrandom::fill(&mut account) {
                    diag::warn!(Sim, "bots: account identity creation failed: {error}");
                    return added;
                }
            }
            let brain = (!dummy)
                .then(|| HostController::new(seed ^ (u64::from(id.0) << 32) ^ u64::from(i)));
            self.bots.push(BotSlot {
                id,
                account: sim::AccountId(account),
                brain,
                joined: false,
                joined_at: None,
                class_picked_in: None,
                class_picks: 0,
                last_cmd: None,
                side,
            });
            added.push(id);
        }
        added
    }

    fn claim_id(&mut self, taken: &[ClientId]) -> Option<ClientId> {
        for _ in 1..MAX_CLIENT_SLOTS {
            if !(1..MAX_CLIENT_SLOTS).contains(&self.next_client) {
                self.next_client = 1;
            }
            let id = ClientId(self.next_client);
            self.next_client += 1;
            if !taken.contains(&id) && !self.is_bot(id) {
                return Some(id);
            }
        }
        None
    }
}

pub fn default_class_index(seed: u64, client: ClientId, pick: u32) -> u8 {
    let mix = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(u64::from(client.0).wrapping_mul(0xBF58_476D_1CE4_E5B9))
        .wrapping_add(u64::from(pick).wrapping_mul(0x94D0_49BB_1331_11EB));
    (mix >> 33) as u8
}
