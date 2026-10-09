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
const GAMEOBJECTS: &str = "maps/mp/gametypes/_gameobjects";

/// The bomb mode's CS C4 threads, put in `sd.gsc` ahead of `initGametypeAwards`:
/// - `csPlantThink` (a site): a carrier holding attack with the C4 in the site starts the plant
///   (the site's use code then runs it: 3 s, frozen, progress bar).
/// - `csBombCarrierThink` (the carrier): attack with the C4 outside a site says where to plant.
/// - `csBombDropThink` (the carrier): `drop` with the C4 in hand (the engine's `cs_drop_bomb`)
///   drops the bomb at the carrier's feet.
const CS_BOMB_FUNCTIONS: &str = r#"csPlantThink()
{
    self endon ( "deleted" );
    for ( ;; )
    {
        wait ( 0.05 );
        if ( self.interactTeam == "none" || ( isDefined( self.inUse ) && self.inUse ) )
            continue;
        foreach ( player in level.players )
        {
            if ( !isDefined( player.carryObject ) || player.carryObject != self.keyObject )
                continue;
            if ( !isReallyAlive( player ) || !player attackButtonPressed() || player getCurrentWeapon() != level.csBomb )
                continue;
            if ( player isTouching( self.trigger ) )
                self.trigger notify ( "trigger", player );
        }
    }
}

csBombCarrierThink( bomb )
{
    self endon ( "death" );
    self endon ( "disconnect" );
    self endon ( "cs_bomb_gone" );
    hinted = false;
    for ( ;; )
    {
        if ( self attackButtonPressed() && self getCurrentWeapon() == level.csBomb && !( isDefined( self.isPlanting ) && self.isPlanting ) )
        {
            inSite = false;
            foreach ( zone in level.bombZones )
            {
                if ( self isTouching( zone.trigger ) )
                    inSite = true;
            }
            if ( !inSite && !hinted )
                self iPrintLnBold( "C4 must be planted at a bomb site!" );
            hinted = true;
        }
        else
        {
            hinted = false;
        }
        wait ( 0.05 );
    }
}

csBombDropThink( bomb )
{
    self endon ( "death" );
    self endon ( "disconnect" );
    self endon ( "cs_bomb_gone" );
    self waittill ( "cs_drop_bomb" );
    if ( isDefined( self.carryObject ) && self.carryObject == bomb && !( isDefined( self.isPlanting ) && self.isPlanting ) )
        bomb thread maps\mp\gametypes\_gameobjects::setDropped();
}

initGametypeAwards()
{"#;

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
        "setClientNameMode( \"manual_change\" ); setDvar( \"cs_bomb\", \"none\" ); \
         makeDvarServerInfo( \"cs_bomb\", \"none\" ); setDvar( \"cs_attackers\", game[\"attackers\"] ); \
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
         csWin = \"draw\"; \
         if ( isDefined( winningTeam ) && winningTeam == game[\"attackers\"] ) csWin = \"t\"; \
         else if ( isDefined( winningTeam ) && winningTeam == game[\"defenders\"] ) csWin = \"ct\"; \
         if ( csWin == \"t\" ) playSoundOnPlayers( \"cs_event_terwin\" ); \
         else if ( csWin == \"ct\" ) playSoundOnPlayers( \"cs_event_ctwin\" ); \
         else playSoundOnPlayers( \"cs_event_rounddraw\" ); \
         csWhy = \"\"; \
         if ( level.bombExploded ) csWhy = \"target_bombed\"; \
         else if ( level.bombDefused ) csWhy = \"bomb_defused\"; \
         else if ( csWin == \"t\" ) csWhy = \"cts_eliminated\"; \
         else if ( csWin == \"ct\" && isDefined( level.aliveCount[game[\"attackers\"]] ) \
         && level.aliveCount[game[\"attackers\"]] > 0 ) csWhy = \"target_saved\"; \
         else if ( csWin == \"ct\" ) csWhy = \"ts_eliminated\"; \
         csEnd = getTime() + \" \" + csWin + \" \" + csWhy; \
         setDvar( \"cs_round_end\", csEnd ); makeDvarServerInfo( \"cs_round_end\", csEnd ); }\n\
         \tthread maps\\mp\\gametypes\\_gamelogic::endGame( winningTeam, endReasonText );",
        "defusal: CS round-end radio voice, and the round's winner and reason for the CS banner \
         (`cs_round_end`)",
    ),
    once(
        "maps/mp/gametypes/_hud_message",
        "\tself thread resetTeamOutcomeNotify( outcomeTitle, outcomeText, leftIcon, rightIcon, \
         leftScore, rightScore, matchBonus );",
        "\toutcomeTitle.sort = 4242; outcomeText.sort = 4242; leftIcon.sort = 4242; \
         rightIcon.sort = 4242; leftScore.sort = 4242; rightScore.sort = 4242; \
         if ( isDefined( matchBonus ) ) matchBonus.sort = 4242;\n\
         \tself thread resetTeamOutcomeNotify( outcomeTitle, outcomeText, leftIcon, rightIcon, \
         leftScore, rightScore, matchBonus );",
        "MW2's round outcome tagged (sort 4242) so a CS round-end banner can stand in for it",
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
        "set3DIcon( \"friendly\", \"waypoint_defend\" + label );",
        "set3DIcon( \"friendly\", undefined );",
        "defusal: no site marker in the world (the radar shows the sites)",
    ),
    once(
        SD,
        "set3DIcon( \"enemy\", \"waypoint_target\" + label );",
        "set3DIcon( \"enemy\", undefined );",
        "defusal: no target marker in the world",
    ),
    once(
        SD,
        "set3DIcon( \"friendly\", \"waypoint_defuse\" + label );",
        "set3DIcon( \"friendly\", undefined );",
        "defusal: no defuse marker over the planted bomb",
    ),
    once(
        SD,
        "set3DIcon( \"enemy\", \"waypoint_defend\" + label );",
        "set3DIcon( \"enemy\", undefined );",
        "defusal: no defend marker over the planted bomb",
    ),
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
    // Changing team kills a living player (`self suicide()`), then moves them (`addToTeam`). The
    // engine settles a scripted death after the script that caused it, so `Callback_PlayerKilled`
    // ran on the new team: the old team kept a living player that wasn't there (a team switched
    // empty never lost the round) and the new one lost one (a 1v1 switch handed the round to the
    // empty side). Waiting a script frame lets the death settle on the old team first, as MW2's
    // `suicide` settles it at once (`waittillframeend` resumes before the engine settles it).
    Patch {
        module: "maps/mp/gametypes/_menus",
        find: "self suicide();",
        replace: "self suicide(); wait ( 0.05 );",
        times: 4,
        why: "team change: the switching player dies on the team they leave",
    },
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
    // CS has no medals or killstreaks: no MW2 splash chime after a kill (the splash itself is
    // hidden by the CS HUD), and no killstreak rewards, so no "Predator missile ready" voice.
    once(
        "maps/mp/gametypes/_hud_message",
        "if ( isDefined( actionData.sound ) )",
        "if ( 0 )",
        "no medal / splash chime",
    ),
    once(
        "maps/mp/gametypes/_hud_message",
        "if ( isDefined( actionData.leaderSound ) )",
        "if ( 0 )",
        "no announcer on splashes (killstreak earned)",
    ),
    once(
        "maps/mp/gametypes/_damage",
        "attacker thread maps\\mp\\killstreaks\\_killstreaks::checkKillstreakReward( \
         attacker.pers[\"cur_kill_streak\"] );",
        "attacker notify( \"got_killstreak\", attacker.pers[\"cur_kill_streak\"] );",
        "no killstreak rewards",
    ),
    once(
        "maps/mp/gametypes/_damagefeedback",
        "updateDamageFeedback( typeHit )\n{",
        "updateDamageFeedback( typeHit )\n{\n\treturn;",
        "no hitmarker or hit sound (CS has none)",
    ),
    // The round timer: no "match ending soon" at a minute left, and the countdown ticks only in
    // the last 10 seconds (MW2 ticked every other second from 30). `snd_timer_warning_volume`
    // sets how loud they are.
    once(
        GAMELOGIC,
        "if ( (timeLeftInt >= 30 && timeLeftInt <= 60) )",
        "if ( 0 )",
        "round timer: no one-minute warning",
    ),
    once(
        GAMELOGIC,
        "if ( timeLeftInt <= 10 || (timeLeftInt <= 30 && timeLeftInt % 2 == 0) )",
        "if ( timeLeftInt <= 10 )",
        "round timer: ticks in the last 10 seconds only",
    ),
    // ---- The CS C4. The bomb mode keeps MW2's sites, timer and round end; the player side is
    // CS 1.6's: the carrier holds the C4 (slot 5, MW2's own `briefcase_bomb_mp`) and plants it
    // by holding attack in a site for 3 s, frozen in place; E only defuses (10 s, 5 s with a
    // defuse kit). `level.csBomb` names the C4 for the generic use-object code below.
    once(
        GAMEOBJECTS,
        "if ( isSubStr( player getCurrentWeapon(), \"killstreak\" ) )",
        "if ( isDefined( level.csBomb ) && isDefined( self.keyObject ) && !( player \
         attackButtonPressed() && player getCurrentWeapon() == level.csBomb ) )\n\
         \t\t\tcontinue;\n\n\
         \t\tif ( isSubStr( player getCurrentWeapon(), \"killstreak\" ) )",
        "CS C4: a site is planted at with attack and the C4 in hand, never with E",
    ),
    once(
        GAMEOBJECTS,
        "player _disableWeapon();",
        "if ( !isDefined( level.csBomb ) ) player _disableWeapon();",
        "CS C4: the weapon stays up while planting (the C4 arms) and defusing",
    ),
    Patch {
        module: GAMEOBJECTS,
        find: "player _enableWeapon();",
        replace: "if ( !isDefined( level.csBomb ) ) player _enableWeapon();",
        times: 2,
        why: "CS C4: nothing to bring back after planting or defusing",
    },
    once(
        GAMEOBJECTS,
        "personalUseBar( object )\n{",
        "personalUseBar( object )\n{\n\tif ( isDefined( level.csBomb ) )\n\t\treturn;",
        "CS C4: the CS HUD draws the plant/defuse bar, not MW2's",
    ),
    once(
        GAMEOBJECTS,
        "player useButtonPressed()",
        "self csUseHeld( player )",
        "CS C4: planting holds attack, defusing holds E",
    ),
    once(
        GAMEOBJECTS,
        "detachUseModels()\n{",
        "csUseHeld( player )\n{\n\
         \tif ( isDefined( level.csBomb ) && isDefined( self.keyObject ) )\n\
         \t\treturn player attackButtonPressed() && player getCurrentWeapon() == level.csBomb;\n\
         \treturn player useButtonPressed();\n}\n\n\
         detachUseModels()\n{",
        "CS C4: which button holds a use",
    ),
    once(
        SD,
        "\tlevel.bombPlanted = false;",
        "\tlevel.csBomb = \"briefcase_bomb_mp\";\n\tlevel.bombPlanted = false;",
        "CS C4: the bomb is a weapon in the carrier's hands",
    ),
    once(
        SD,
        "bombZone.useWeapon = \"briefcase_bomb_mp\";",
        "bombZone.useWeapon = undefined;\n\t\tbombZone thread csPlantThink();",
        "CS C4: holding attack with the C4 in a site starts the plant",
    ),
    once(
        SD,
        "defuseObject.useWeapon = \"briefcase_bomb_defuse_mp\";",
        "defuseObject.useWeapon = undefined;",
        "CS C4: defusing keeps the gun in hand",
    ),
    once(
        SD,
        "player playSound( \"mp_bomb_defuse\" );",
        "player playSound( \"cs_c4_disarm\" );\n\
         \t\tif ( player csHasDefuseKit() ) player.objectiveScaler = 2; \
         else player.objectiveScaler = 1;",
        "CS C4: defusing takes 10 s, 5 s with a defuse kit",
    ),
    once(
        SD,
        "\t\tif ( isDefined( level.sdBombModel ) )\n\t\t\tlevel.sdBombModel hide();",
        "\t\tif ( isDefined( level.sdBombModel ) && !isDefined( level.csBomb ) )\n\
         \t\t\tlevel.sdBombModel hide();",
        "CS C4: the bomb stays in sight while it is defused",
    ),
    once(
        SD,
        "bombZone maps\\mp\\gametypes\\_gameobjects::setUseHintText( &\"PLATFORM_HOLD_TO_PLANT_EXPLOSIVES\" );",
        "",
        "CS C4: no \"hold E to plant\" hint (the C4 plants with attack)",
    ),
    once(
        SD,
        "level.sdBomb maps\\mp\\gametypes\\_gameobjects::setCarryIcon( \"hud_suitcase_bomb\" );",
        "",
        "CS C4: no MW2 bomb icon on the carrier's HUD (the CS HUD shows the C4)",
    ),
    once(
        SD,
        "player.isPlanting = true;",
        "player.isPlanting = true;\n\t\tplayer.objectiveScaler = 1;",
        "CS C4: planting takes 3 s",
    ),
    once(
        SD,
        "player iPrintLnBold( &\"MP_CANT_PLANT_WITHOUT_BOMB\" );",
        "if ( !isDefined( level.csBomb ) ) player iPrintLnBold( &\"MP_CANT_PLANT_WITHOUT_BOMB\" );",
        "CS C4: no MW2 \"can't plant without the bomb\" on E in a site",
    ),
    once(
        SD,
        "player playSound( \"mp_bomb_plant\" );",
        "player playSound( \"cs_c4_plant\" );",
        "CS C4: plant sound",
    ),
    once(
        SD,
        "leaderDialog( \"bomb_planted\" );",
        "playSoundOnPlayers( \"cs_event_bombplanted\" );",
        "CS C4: \"The bomb has been planted\" on the radio",
    ),
    once(
        SD,
        "leaderDialog( \"bomb_defused\" );",
        "playSoundOnPlayers( \"cs_event_bombdefused\" ); player playSound( \"cs_c4_disarmed\" );",
        "CS C4: \"The bomb has been defused\" on the radio",
    ),
    once(
        SD,
        "player.isBombCarrier = true;",
        "player.isBombCarrier = true;\n\
         \tif ( isDefined( level.csBomb ) )\n\t{\n\
         \t\tplayer giveWeapon( level.csBomb );\n\
         \t\tplayer thread csBombDropThink( self );\n\
         \t\tplayer thread csBombCarrierThink( self );\n\t}",
        "CS C4: the carrier gets the C4 on slot 5",
    ),
    once(
        SD,
        "leaderDialog( \"bomb_taken\", player.pers[\"team\"] );",
        "",
        "CS C4: no MW2 announcer on taking the bomb",
    ),
    once(
        SD,
        "\tmaps\\mp\\_utility::playSoundOnPlayers( game[\"bomb_dropped_sound\"], game[\"attackers\"] );",
        "",
        "CS C4: no MW2 sound on dropping the bomb",
    ),
    once(
        SD,
        "\tmaps\\mp\\_utility::playSoundOnPlayers( game[\"bomb_recovered_sound\"], game[\"attackers\"] );",
        "",
        "CS C4: no MW2 sound on taking the bomb",
    ),
    once(
        SD,
        "onDrop( player )\n{",
        "onDrop( player )\n{\n\
         \tif ( isDefined( player ) && isDefined( level.csBomb ) )\n\t{\n\
         \t\tplayer notify ( \"cs_bomb_gone\" );\n\
         \t\tif ( isAlive( player ) )\n\t\t\tplayer takeWeapon( level.csBomb );\n\t}",
        "CS C4: dropping (or planting) the bomb takes the C4",
    ),
    once(
        SD,
        "destroyedObj.visuals[0] thread maps\\mp\\gametypes\\_gamelogic::playTickingSound();",
        "",
        "CS C4: no MW2 ticking (CS's beeps instead)",
    ),
    // Clients beep the planted bomb in their own install's rhythm (CS:S speeds one beep up,
    // CS 1.6 steps through five), hide the round timer and draw the planted C4: the plant time,
    // the timer and where it lies.
    once(
        SD,
        "\tBombTimerWait();",
        "\tcsBombInfo = \"planted \" + getTime() + \" \" + level.bombTimer + \" \" + \
         level.sdBombModel.origin[0] + \" \" + level.sdBombModel.origin[1] + \" \" + \
         level.sdBombModel.origin[2];\n\
         \tsetDvar( \"cs_bomb\", csBombInfo ); makeDvarServerInfo( \"cs_bomb\", csBombInfo );\n\
         \tBombTimerWait();",
        "CS C4: clients know the bomb is planted: when, for how long, where",
    ),
    once(
        SD,
        "level.bombExploded = true;",
        "level.bombExploded = true;\n\
         \tsetDvar( \"cs_bomb\", \"exploded\" ); makeDvarServerInfo( \"cs_bomb\", \"exploded\" );",
        "CS C4: clients know the bomb went off",
    ),
    // CS 1.6 plants (and defuses) after the round is decided too, during the pause before the
    // next round: the bomb sites keep working, and a late plant or defuse leaves the round's
    // result and money alone.
    once(
        GAMEOBJECTS,
        "useObjectUseThink()\n{\n\tlevel endon ( \"game_ended\" );",
        "useObjectUseThink()\n{",
        "CS C4: bomb sites work in the pause after a round is decided",
    ),
    once(
        GAMEOBJECTS,
        "useHoldThinkLoop( player, lastWeapon )\n{\n\tlevel endon ( \"game_ended\" );",
        "useHoldThinkLoop( player, lastWeapon )\n{",
        "CS C4: a plant or defuse in that pause runs to the end",
    ),
    once(
        SD,
        "\tlevel.bombPlanted = true;",
        "\tif ( !level.gameEnded ) level.bombPlanted = true;",
        "CS C4: a plant after the round is decided pays nothing",
    ),
    once(
        SD,
        "level.bombDefused = true;",
        "if ( !level.gameEnded ) level.bombDefused = true;\n\
         \tsetDvar( \"cs_bomb\", \"defused\" ); makeDvarServerInfo( \"cs_bomb\", \"defused\" );",
        "CS C4: clients know the bomb was defused",
    ),
    Patch {
        module: SD,
        find: "explosionOrigin, 512, 200, 20",
        replace: "explosionOrigin, 1750, 500, 0",
        times: 2,
        why: "CS C4: 500 damage falling off to nothing at 1750 units",
    },
    once(
        SD,
        "\"exp_suitcase_bomb_main\"",
        "\"cs_c4_explode\"",
        "CS C4: explosion sound",
    ),
    once(
        SD,
        "initGametypeAwards()\n{",
        CS_BOMB_FUNCTIONS,
        "CS C4: plant, carry, drop and beep threads",
    ),
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
