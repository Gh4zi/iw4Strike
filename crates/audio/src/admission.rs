#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LimitMode {
    #[default]
    Unlimited,
    Oldest,
    Reject,
    Priority,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AliasLimit {
    pub mode: LimitMode,
    pub count: u8,
    pub per_emitter: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ChannelRule {
    pub id: u32,
    pub maximum: i32,
    pub restricted: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AdmissionPolicy {
    pub bank_revision: u64,
    pub alias: Option<usize>,
    pub emitter: Option<u32>,
    pub channel: Option<ChannelRule>,
    pub limits: [AliasLimit; 2],
    pub priority: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AdmissionFailure {
    QueueFull = 1,
    LogicalBudget,
    PhysicalBudget,
    Concurrency,
    Cancelled,
    StaleScope,
}

pub(crate) struct Occupant {
    pub id: u64,
    pub policy: AdmissionPolicy,
    pub priority: f32,
}

pub(crate) fn plan(
    request: &AdmissionPolicy,
    occupants: &[Occupant],
) -> Result<Vec<u64>, AdmissionFailure> {
    let mut victims = std::collections::BTreeSet::new();
    let same_bank = |occupant: &Occupant| occupant.policy.bank_revision == request.bank_revision;
    if let Some(channel) = request.channel {
        if channel.maximum <= 0 {
            return Err(AdmissionFailure::Concurrency);
        }
        if channel.restricted && request.emitter.is_some() {
            victims.extend(
                occupants
                    .iter()
                    .filter(|occupant| {
                        same_bank(occupant)
                            && occupant.policy.emitter == request.emitter
                            && occupant
                                .policy
                                .channel
                                .is_some_and(|old| old.id == channel.id)
                    })
                    .map(|occupant| occupant.id),
            );
        }
    }
    if request.alias.is_some() {
        for limit in request.limits {
            if limit.mode == LimitMode::Unlimited {
                continue;
            }
            if limit.count == 0 {
                return Err(AdmissionFailure::Concurrency);
            }
            let mut candidates: Vec<_> = occupants
                .iter()
                .filter(|occupant| {
                    same_bank(occupant)
                        && occupant.policy.alias == request.alias
                        && (!limit.per_emitter || occupant.policy.emitter == request.emitter)
                        && !victims.contains(&occupant.id)
                })
                .collect();
            let excess = candidates
                .len()
                .saturating_add(1)
                .saturating_sub(usize::from(limit.count));
            if excess == 0 {
                continue;
            }
            match limit.mode {
                LimitMode::Oldest => candidates.sort_unstable_by_key(|occupant| occupant.id),
                LimitMode::Priority => candidates.sort_unstable_by(|a, b| {
                    a.priority.total_cmp(&b.priority).then(a.id.cmp(&b.id))
                }),
                _ => return Err(AdmissionFailure::Concurrency),
            }
            for victim in candidates.into_iter().take(excess) {
                if limit.mode == LimitMode::Priority && request.priority <= victim.priority + 3.0 {
                    return Err(AdmissionFailure::Concurrency);
                }
                victims.insert(victim.id);
            }
        }
    }
    if let Some(channel) = request.channel {
        let remaining = occupants
            .iter()
            .filter(|occupant| {
                same_bank(occupant)
                    && occupant
                        .policy
                        .channel
                        .is_some_and(|old| old.id == channel.id)
                    && !victims.contains(&occupant.id)
            })
            .count();
        if remaining >= channel.maximum as usize {
            return Err(AdmissionFailure::Concurrency);
        }
    }
    Ok(victims.into_iter().collect())
}
