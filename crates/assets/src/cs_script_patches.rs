//! Counter-Strike fork edits to MW2's own scripts, applied to their source text as it loads
//! (CS rules only). Each patch replaces an exact piece of a line, once (or at every occurrence
//! when `times` says so); one that no longer matches as expected is logged and skipped, so a
//! changed script fails loudly instead of silently.

struct Patch {
    module: &'static str,
    find: &'static str,
    replace: &'static str,
    /// How many times `find` must occur; every occurrence is replaced.
    times: usize,
    why: &'static str,
}

const fn once(
    module: &'static str,
    find: &'static str,
    replace: &'static str,
    why: &'static str,
) -> Patch {
    Patch {
        module,
        find,
        replace,
        times: 1,
        why,
    }
}

const GAMELOGIC: &str = "maps/mp/gametypes/_gamelogic";
const SD: &str = "maps/mp/gametypes/sd";

const PATCHES: &[Patch] = &[
    once(
        "maps/mp/gametypes/_gamescore",
        "if ( !player rankingEnabled() && !level.hardcoreMode )",
        "if ( 0 )",
        "no \"+50\" score popups (CS shows none)",
    ),
    once(
        "maps/mp/_utility",
        "if ( isDefined( self.perks[perkName] ) )",
        "if ( 0 )",
        "no perks: `_hasPerk` is always false (the class perks the scripts still record \
         cancelled fall damage through Commando Pro, among others)",
    ),
    // Defusal (Search and Destroy) in CS2's competitive format: first to 13 rounds, sides switch
    // after 12, 1:55 rounds, 3 s plant, 10 s defuse, 40 s bomb. MW2 caps the limits at 12 and 9.
    once(
        SD,
        "registerRoundSwitchDvar( level.gameType, 3, 0, 9 );",
        "registerRoundSwitchDvar( level.gameType, 12, 0, 30 );",
        "defusal: sides switch after 12 rounds",
    ),
    once(
        SD,
        "registerTimeLimitDvar( level.gameType, 2.5, 0, 1440 );",
        "registerTimeLimitDvar( level.gameType, 1.9167, 0, 1440 );",
        "defusal: 1:55 rounds",
    ),
    once(
        SD,
        "registerRoundLimitDvar( level.gameType, 0, 0, 12 );",
        "registerRoundLimitDvar( level.gameType, 24, 0, 30 );",
        "defusal: at most 24 rounds",
    ),
    once(
        SD,
        "registerWinLimitDvar( level.gameType, 4, 0, 12 );",
        "registerWinLimitDvar( level.gameType, 13, 0, 30 );",
        "defusal: first to 13 rounds wins",
    ),
    once(
        SD,
        "level.plantTime = dvarFloatValue( \"planttime\", 5, 0, 20 );",
        "level.plantTime = dvarFloatValue( \"planttime\", 3, 0, 20 );",
        "defusal: 3 s plant",
    ),
    once(
        SD,
        "level.defuseTime = dvarFloatValue( \"defusetime\", 5, 0, 20 );",
        "level.defuseTime = dvarFloatValue( \"defusetime\", 10, 0, 20 );",
        "defusal: 10 s defuse",
    ),
    once(
        SD,
        "setClientNameMode( \"manual_change\" );",
        "setClientNameMode( \"manual_change\" ); setDvar( \"cs_attackers\", game[\"attackers\"] ); \
         makeDvarServerInfo( \"cs_attackers\", game[\"attackers\"] );",
        "defusal: tell clients which team attacks (the Terrorists), each round",
    ),
    // CS's radio voice ends every round: "Terrorists win!", "Counter-Terrorists win!", "Round
    // draw!". The client plays whichever install it has (`asset_audio::CS_EVENT_PREFIX`). Only
    // the call that decides the round speaks: a death after it (the other team's last player
    // killed during the round end) calls this again, and `endGame` ignores that call.
    once(
        SD,
        "\tthread maps\\mp\\gametypes\\_gamelogic::endGame( winningTeam, endReasonText );",
        "\tif ( !level.gameEnded ) { \
         if ( isDefined( winningTeam ) && winningTeam == game[\"attackers\"] ) \
         playSoundOnPlayers( \"cs_event_terwin\" ); \
         else if ( isDefined( winningTeam ) && winningTeam == game[\"defenders\"] ) \
         playSoundOnPlayers( \"cs_event_ctwin\" ); \
         else playSoundOnPlayers( \"cs_event_rounddraw\" ); }\n\
         \tthread maps\\mp\\gametypes\\_gamelogic::endGame( winningTeam, endReasonText );",
        "defusal: CS round-end radio voice",
    ),
    once(
        "maps/mp/gametypes/_music_and_dialog",
        "\tlevel waittill ( \"round_win\", winner );",
        "\tlevel waittill ( \"round_win\", winner );\n\treturn;",
        "no MW2 \"round won/lost\" announcer over CS's radio voice",
    ),
    // CS shows the bomb (carried or dropped) to the Terrorists on the radar only: no "escort" or
    // bomb marker floating in the world.
    once(
        SD,
        "self maps\\mp\\gametypes\\_gameobjects::set3DIcon( \"friendly\", \"waypoint_escort\" );",
        "self maps\\mp\\gametypes\\_gameobjects::set3DIcon( \"friendly\", undefined );",
        "defusal: no escort marker over the bomb carrier",
    ),
    Patch {
        module: SD,
        find: "_gameobjects::set3DIcon( \"friendly\", \"waypoint_bomb\" );",
        replace: "_gameobjects::set3DIcon( \"friendly\", undefined );",
        times: 2,
        why: "defusal: no world marker over the dropped bomb",
    },
    once(
        SD,
        "level.bombTimer = dvarFloatValue( \"bombtimer\", 45, 1, 300 );",
        "level.bombTimer = dvarFloatValue( \"bombtimer\", 40, 1, 300 );",
        "defusal: 40 s bomb",
    ),
    // CS2 switches sides once, at halftime (after round 12); MW2 switched at every multiple of the
    // round switch.
    once(
        GAMELOGIC,
        "if ( game[\"roundsPlayed\"] % level.roundSwitch == 0 )",
        "if ( game[\"roundsPlayed\"] == level.roundSwitch )",
        "defusal: sides switch once, at halftime",
    ),
    // A team wiped out ends the round at once, as in CS. MW2 skipped the check for the first 15 s
    // of a round (its grace period, which still lets round-start stragglers spawn); a team with a
    // player yet to spawn still has lives, so it isn't counted as dead.
    once(
        GAMELOGIC,
        "\tif ( level.inGracePeriod )\n\t\treturn;\n\n\tif ( level.teamBased )\n\t{\n\t\tlivesCount",
        "\tif ( level.teamBased )\n\t{\n\t\tlivesCount",
        "defusal: a team wiped out in the first 15 s ends the round at once",
    ),
    // A decided round is MW2's "postgame", where nobody takes damage and a death skips the kill
    // feed and score. CS fights on until the next round (exit frags); only the match's end
    // stops it.
    Patch {
        module: "maps/mp/gametypes/_damage",
        find: "if ( game[ \"state\" ] == \"postgame\" )",
        replace: "if ( game[ \"state\" ] == \"postgame\" && wasLastRound() )",
        times: 2,
        why: "round end: players can still be hurt and killed until the next round",
    },
    // CS round ends: the round is decided, everyone keeps moving for 7 s (escape the bomb, save a
    // gun), no freeze, blur or outro look until the match itself is over.
    once(
        GAMELOGIC,
        "level.roundEndDelay = 4;",
        "level.roundEndDelay = 7;",
        "round end: 7 s before the next round, like CS2",
    ),
    once(
        GAMELOGIC,
        "player thread freezePlayerForRoundEnd( 1.0 );",
        "if ( wasLastRound() || ( level.teamBased && isDefined( level.roundSwitch ) && \
         level.roundSwitch && game[\"roundsPlayed\"] == level.roundSwitch ) ) \
         player thread freezePlayerForRoundEnd( 1.0 );",
        "round end: players move freely, except at halftime (sides switch) and the match's end, \
         where they stand frozen like CS's freeze time (look around, drop weapons)",
    ),
    Patch {
        module: GAMELOGIC,
        find: "player thread roundEndDoF( 4.0 );",
        replace: "if ( wasLastRound() ) player thread roundEndDoF( 4.0 );",
        times: 3,
        why: "round end: no blur until the match is over",
    },
    Patch {
        module: GAMELOGIC,
        find: "visionSetNaked( \"mpOutro\", 0.5 );",
        replace: "if ( wasLastRound() ) visionSetNaked( \"mpOutro\", 0.5 );",
        times: 2,
        why: "round end: no outro look until the match is over",
    },
    // CS freeze time: every round after the first opens with `scr_cs_freezetime` seconds frozen in
    // place (MW2's pre-match freeze; the engine still lets you look, switch, drop and buy), the
    // round timer counting it down. No grey "intro" look on any countdown.
    once(
        GAMELOGIC,
        "level.prematchPeriod = 0;",
        "level.prematchPeriod = getDvarInt( \"scr_cs_freezetime\" );",
        "freeze time at the start of every round",
    ),
    once(
        GAMELOGIC,
        "matchStartTimerPC();",
        "if ( game[\"roundsPlayed\"] > 0 ) { setGameEndTime( getTime() + level.prematchPeriod \
         * 1000 ); wait ( level.prematchPeriod ); } else matchStartTimerPC();",
        "freeze time: a silent countdown after the first round",
    ),
    once(
        GAMELOGIC,
        "if ( !gameFlag( \"prematch_done\" ) )\n\t{\n\t\tsetGameEndTime( 0 );",
        "if ( !gameFlag( \"prematch_done\" ) )\n\t{\n\t\tif ( game[\"roundsPlayed\"] == 0 ) \
         setGameEndTime( 0 );",
        "freeze time: the time-limit check leaves the freeze countdown on the round timer",
    ),
    Patch {
        module: GAMELOGIC,
        find: "visionSetNaked( \"mpIntro\", 0 );",
        replace: "visionSetNaked( getDvar( \"mapname\" ), 0 );",
        times: 2,
        why: "no grey intro look during the countdowns",
    },
    // Final killcam only for the kill that wins the match, not every round's last kill, and
    // only while killcams are on (`scr_game_allowkillcam`, off in the CS fork for now).
    once(
        "maps/mp/gametypes/_damage",
        "if ( isDefined( attacker.finalKill ) && doKillcam && !isDefined( level.nukeDetonated ) )",
        "if ( level.killcam && isDefined( attacker.finalKill ) && doKillcam \
         && !isDefined( level.nukeDetonated ) \
         && ( !level.teamBased || isLastRound() || ( isDefined( attacker.pers ) \
         && game[\"roundsWon\"][attacker.pers[\"team\"]] >= getWatchedDvar( \"winlimit\" ) - 1 ) ) )",
        "final killcam on the match-winning kill only",
    ),
];

/// Applies every patch for `module` to its source.
pub(crate) fn apply(module: &str, bytes: &mut Vec<u8>) {
    if !movement_iw4::rules::CS_RULES {
        return;
    }
    for patch in PATCHES.iter().filter(|p| p.module.eq_ignore_ascii_case(module)) {
        // Byte-wise: some MW2 scripts carry stray non-UTF-8 bytes in their comments.
        let find = patch.find.as_bytes();
        let hits = bytes
            .windows(find.len())
            .enumerate()
            .filter(|(_, window)| *window == find)
            .map(|(at, _)| at)
            .collect::<Vec<_>>();
        if hits.len() == patch.times {
            // Back to front, so earlier offsets stay valid.
            for &at in hits.iter().rev() {
                bytes.splice(at..at + find.len(), patch.replace.bytes());
            }
            diag::info!(Zone, "cs script patch: {module} — {}", patch.why);
        } else {
            diag::warn!(
                Zone,
                "cs script patch: {module} — `{}` found {} times (expected {}), not applied ({})",
                patch.find,
                hits.len(),
                patch.times,
                patch.why
            );
        }
    }
}
