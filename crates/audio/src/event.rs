use std::collections::HashMap;
use std::sync::Mutex;

use crate::CueFailure;

const EVENT_CAPACITY: usize = 8192;
const EVENT_WINDOW_TICKS: u32 = sim::ticks_for_ms(5000);
const FUTURE_TICKS: u32 = sim::ticks_for_ms(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AudioEventId {
    pub world: Option<u64>,
    pub timeline: u64,
    pub emitter: u64,
    pub occurrence: AudioOccurrence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AudioOccurrence {
    Entity {
        domain: net::EntityEventDomain,
        sequence: u32,
        tick: u32,
        ordinal: u16,
    },
    Animation(AnimationMarkerId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AnimationMarkerId {
    pub controller: u64,
    pub playback: u64,
    pub node: usize,
    pub cycle: i64,
    pub marker: usize,
    pub hand: u8,
    pub life: u32,
    pub weapon: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AudioEvent {
    pub id: AudioEventId,
    pub tick: u32,
}

impl AudioEvent {
    pub fn from_entity(
        generation: frame::WorldGeneration,
        entity: bevy::prelude::Entity,
        event: &net::DispatchedEntityEvent,
        ordinal: u16,
    ) -> Self {
        Self {
            id: AudioEventId {
                world: generation.0,
                timeline: event.timeline,
                emitter: entity.to_bits(),
                occurrence: AudioOccurrence::Entity {
                    domain: event.domain,
                    sequence: event.sequence.0,
                    tick: event.tick.0,
                    ordinal,
                },
            },
            tick: event.tick.0,
        }
    }

    pub fn from_animation(
        generation: frame::WorldGeneration,
        timeline: u64,
        client: sim::ClientId,
        tick: sim::Tick,
        marker: AnimationMarkerId,
    ) -> Self {
        Self {
            id: AudioEventId {
                world: generation.0,
                timeline,
                emitter: u64::from(client.0),
                occurrence: AudioOccurrence::Animation(marker),
            },
            tick: tick.0,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct EventContext {
    pub local_life: Option<(u32, u32)>,
    pub world: u64,
    pub timeline: u64,
    pub tick: u32,
}

#[derive(Default)]
pub(crate) struct EventContextState(Mutex<Option<EventContext>>);

impl EventContextState {
    pub(crate) fn set(&self, context: Option<EventContext>) {
        *self.0.lock().unwrap_or_else(|poison| poison.into_inner()) = context;
    }

    pub(crate) fn get(&self) -> Option<EventContext> {
        *self.0.lock().unwrap_or_else(|poison| poison.into_inner())
    }
}

pub(crate) struct EventJournal {
    available: bool,
    context: Option<EventContext>,
    accepted: HashMap<AudioEventId, u32>,
}

impl EventJournal {
    pub(crate) fn new() -> Self {
        Self {
            available: false,
            context: None,
            accepted: HashMap::with_capacity(EVENT_CAPACITY),
        }
    }

    pub(crate) fn advance(&mut self, context: Option<EventContext>) {
        self.available = context.is_some();
        if !self.available {
            return;
        }
        let same_timeline = self
            .context
            .zip(context)
            .is_some_and(|(old, new)| old.world == new.world && old.timeline == new.timeline);
        if !same_timeline {
            self.accepted.clear();
            self.context = context;
        } else if let (Some(old), Some(mut new)) = (self.context, context) {
            if new.tick.wrapping_sub(old.tick) >= 1 << 31 {
                new.tick = old.tick;
            }
            self.context = Some(new);
            if new.tick == old.tick {
                return;
            }
            self.accepted.retain(|_, tick| {
                let age = new.tick.wrapping_sub(*tick);
                age < EVENT_WINDOW_TICKS || age >= 1 << 31
            });
        }
    }

    pub(crate) fn current(&self, event: Option<AudioEvent>) -> bool {
        event.is_none_or(|event| {
            self.available
                && self.context.is_some_and(|context| {
                    event.id.world == Some(context.world)
                        && event.id.timeline == context.timeline
                        && match event.id.occurrence {
                            AudioOccurrence::Entity { .. } => true,
                            AudioOccurrence::Animation(marker) => {
                                context.local_life.is_some_and(|(client, life)| {
                                    u64::from(client) == event.id.emitter && life == marker.life
                                })
                            }
                        }
                })
        })
    }

    pub(crate) fn accept(&mut self, event: Option<AudioEvent>) -> Result<(), CueFailure> {
        let Some(event) = event else {
            return Ok(());
        };
        if !self.available {
            return Err(CueFailure::StaleEvent);
        }
        let Some(context) = self.context else {
            return Err(CueFailure::StaleEvent);
        };
        let id = event.id;
        if !self.current(Some(event)) {
            return Err(CueFailure::StaleEvent);
        }
        let age = context.tick.wrapping_sub(event.tick);
        if age >= EVENT_WINDOW_TICKS && age < 1 << 31 {
            return Err(CueFailure::StaleEvent);
        }
        if age >= 1 << 31 && event.tick.wrapping_sub(context.tick) > FUTURE_TICKS {
            return Err(CueFailure::StaleEvent);
        }
        if self.accepted.contains_key(&id) {
            return Err(CueFailure::DuplicateEvent);
        }
        if self.accepted.len() == EVENT_CAPACITY {
            return Err(CueFailure::EventBudget);
        }
        self.accepted.insert(id, event.tick);
        Ok(())
    }
}
