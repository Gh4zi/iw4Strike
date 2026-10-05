# IW4L — personal fork notes

## What this is
IW4L (vladtrc/iw4L): a from-scratch Rust/Bevy/wgpu runtime for Call of Duty:
Modern Warfare 2 (2009), reading retail MW2 (and MW3/Black Ops) fastfiles
natively. No upstream releases — build from source only. Whole project is
LLM-written; nothing is API-stable between commits.

Build/run reference:
```
cp .env.example .env         # set IW4L_GAMES to the folder with the game install
make map mp_boneyard CMDS='wait world; spawn 0; force_match_start; bot add 3'
```
(Windows: GNU Make via winget `GnuWin32.Make`, or skip Make and run
`cargo run --profile play -p launcher -- map mp_boneyard` directly.)

Environment: Windows, Rust via rustup, Visual Studio Build Tools (Desktop
development with C++) for the MSVC linker. Build confirmed working —
mp_boneyard launches, menus/minimap/viewmodel render correctly.

## Goal
**Updated 2026-10-04: full CS 1.6 port onto MW2 maps.** Not "CS-style
feel" — port CS 1.6 itself: movement (bhop, air-strafe, surf, duck-jump),
player health/armor, damage & hitboxes, knife, weapons, and the game
mechanics/utilities (rounds, buy, bomb, money). MW2 supplies maps and the
engine runtime; gameplay becomes CS 1.6. Suggested order: movement →
health/armor/damage/hitboxes → knife → weapons (needs converted assets) →
round/buy/bomb mechanics. Surf needs surf ramps; MW2 maps have none, so it
works by physics but needs custom geometry/maps later.

Original goal notes:
Recreate Half-Life/Counter-Strike-style gameplay *inside* IW4L, on IW4L's own
native MW2 maps — not by combining with another engine (ruled out Xash3D:
unrelated GoldSrc format, no shared lineage with IW4's fastfiles; the Source
SDK angle is useful only as a movement-math *reference*, not an engine to
integrate).

Three sub-projects, roughly in priority order:

### 1. CS-style tick rate (biggest structural change)
IW4L's authority/gameplay tick is fixed at 20Hz (50ms) — inherited directly
from retail CoD4-engine convention. GSC's `wait` scheduling is explicitly
defined as "20 frames per second" (see docs/GSC-RUNTIME.md), and
`SetSlowMotion` describes "the 50 ms gameplay ticks." A separate 17ms physics
accumulator exists but looks like it's for movers/props, not the core
movement/hit-reg path (movement natives only run on authority frames, per
docs/SIM-STEP.md).

Target: 64 or 100Hz authority tick, CS-equivalent precision.

Planned phases:
1. **Baseline** — confirm `make map mp_boneyard ...` works (done). Locate the
   real call site: `sim::step` called with `StepReason::AuthorityFrame`
   inside `net::authority::runtime` (per docs/SIM-STEP.md), and the GSC
   `wait` bucket scheduler under `sim::script` (per docs/GSC-RUNTIME.md).
2. **Decouple the script clock from the tick clock** (do this before
   anything else) — add a raw-tick counter and a separate script-frame
   counter that only advances every Nth raw tick (N = new_rate / 20), so
   GSC's `wait`/`SetSlowMotion` timing semantics stay correct at 1 "frame" =
   1/20s regardless of raw tick rate. Validate with a stopwatch against a
   scripted timer before moving on.
   *Status (2026-10-04): code done, awaiting in-game check.* The scheduler
   already keys buckets by game milliseconds (`now = tick * MATCH_TICK_MS`,
   due = `range(..=now)`), so no separate counter was needed: `wait` now
   converts frames with `sim::SCRIPT_FRAME_MS = 50` (`crates/sim/src/score.rs`)
   instead of `MATCH_TICK_MS`. Identical at 20Hz; `cargo check -p sim` passes.
   Tick constants: sim `MATCH_TICK_MS = 50` (`score.rs`, 43 uses, plus a
   `const assert == 50` in `sim/src/damage.rs`) and net `AUTHORITY_HZ`/
   `AUTHORITY_MS` (`crates/net/src/authority/inbox.rs`, 35 uses).
   Design constraint: game time is integer ms, so 100Hz (10ms) fits; 64Hz
   (15.625ms) would need a time-representation rewrite.
   **Decision (2026-10-04): 100Hz (10ms).** Reasons: integer ms fits the
   engine's time model (64Hz = 15.625ms doesn't); 100 is the classic
   CS 1.6/HL server tickrate (`sys_ticrate 100`), the exact cadence the
   bhop/air-strafe math was tuned on; higher tick = smoother, more
   consistent strafing than 64.
3. **Raise the raw authority tick rate** to 100Hz once step 2 is proven
   stable. Also audit movement/weapon-trace code for other literal
   `50`/`msec`-assumes-20Hz constants. Known so far, not tied to
   `AUTHORITY_MS`: `TICK_MS = 50` (`crates/bots/src/weapon.rs`),
   `SERVER_FRAME_MS = 50` / `SERVER_FRAME_SECONDS = 0.05`
   (`crates/killcam_iw4/src/camtime.rs`), and bot timings counted in ticks
   (`crates/bots/src/controller.rs`, `memory.rs`, `plugin.rs`).
4. **Scale the netcode** — snapshot rate 20Hz→64-100Hz is 3-5x more UDP
   traffic through the custom delta/reliability layer; client
   prediction/reconciliation (`net::client::predict`, `adopt_prediction_snapshot`)
   must agree with authority on the new cadence. Expect this to be the
   biggest unknown.
5. **Fix replay/determinism** — the determinism test suite and demo/replay
   format are keyed to current tick semantics; expect fixture updates and
   old demos to desync.

### 2. CS-style movement and shooting feel
- Round structure / bomb plant-defuse: build on IW4's existing Search &
  Destroy / Sabotage modes (already CoD's own CS-style round mode). Engine
  status notes full plant/defuse isn't fully wired yet — extend, don't start
  from scratch. This is GSC scripting (gameplay policy lives entirely in
  loaded GSC, not Rust — see docs/GSC-RUNTIME.md), not an engine change.
- Weapon damage/TTK tuning: weapon-table data + GSC natives, no engine work.
  Check whether MW2's sniper is already hitscan (CoD bullet weapons usually
  are, same as CS — if so, no work needed there).
- CS-style accuracy model (degrades on sustained fire, recovers over time,
  worsens while moving/jumping) is genuinely new scripting — different
  mechanic from MW2's recoil-kick system. Needs GSC reading movement/weapon
  state each tick (natives like `IsReloading`, `GetCurrentWeaponClipAmmo`
  exist; velocity-based accuracy penalty not confirmed in the native list).
- **Flavor (2026-10-04): CS 1.6 first, CS:Source later if it goes well.**
  Keep every tunable (accel, airaccel, friction, stopspeed, jump height,
  bhop cap/landing slowdown, duck speeds, per-weapon maxspeed) in one
  movement-profile struct, so Source is a second profile + small rule
  diffs, not a rewrite. References (read-only, in `context/externals/`):
  hlsdk-portable `pm_shared` (HL base movement) and ReGameDLL_CS (CS 1.6
  rules). Source SDK 2013 `gamemovement.cpp` for the later Source profile
  (CS:S game code was never released — don't use leaks). Read formulas,
  write our own Rust; never copy their code (licenses).
- **Scope (2026-10-04): rework the whole player movement into CS movement**,
  not just bolt on air-strafing — ground accel + friction, air accel with
  the 30u/s wishspeed cap (what makes air-strafe/bhop work), jump without
  speed loss when jumping on landing, CS speeds per weapon, crouch/duck-jump,
  no sprint/slide. Bhop math must stay correct at the 100Hz tick (it's
  frametime-dependent). Old note follows:
- Bunny-hop/air-strafe movement feel is the one part needing real Rust
  changes: CoD's movement (sprint, ADS, mantle, slide-movers) is Rust-side
  primitives GSC can only toggle (`AllowSprint`, `AllowADS`, `CanMantle`),
  not redefine. Valve's `source-sdk-2013` (GitHub, HL2/HL2:DM/TF2 game code)
  has the actual CS/HL air-accel and friction math as a reference — read it
  to understand the formulas, then write an equivalent clean Rust
  implementation. Don't port the licensed C++ directly (Source SDK's license
  is free-use/non-commercial only, incompatible with IW4L's Apache-2.0).

### 3. Literal CS weapon assets (Deagle, AWP) — optional, lower priority
Not a new engine reader — convert and reuse IW4's existing iw4-format
pipeline:
1. Crowbar + Blender to decompile CS:Source's `.mdl`/VTF/sound assets.
2. Retarget mesh + animations to MW2's viewmodel skeleton (the slow part —
   different rig, different anim data).
3. Compile into a real `.ff` zone with OpenAssetTools' zonebuilder (the same
   toolkit IW4L's own authors credit for its asset-layout knowledge) — once
   compiled this way, IW4L loads it like any native weapon, no new reader
   code.
4. Wire in the damage/accuracy tuning from sub-project 2.
Personal-use conversion of assets from games you own is fine; redistributing
the converted result bundles both Valve's and Activision's IP without rights
to either — don't publish it.

## Engine decision (2026-10-04): stay on IW4L ("option C")
Not merging engines (Source engine is closed binaries; converting MW2 maps
into Source = weeks/map + asset-redistribution problem). Port gameplay
logic into IW4L: **Momentum Mod movement (CS:GO style), CS 1.6/Source SDK
gameplay + weapons**. User authorised long autonomous sessions (hours,
away from PC): no questions mid-run, record every step here.

## CS 1.6 assets for weapons (user's own install)
Path: `C:\Program Files (x86)\Steam\steamapps\common\Half-Life\cstrike`
(latest Steam build; loose files, no .pak: `models/` 155 files incl.
`v_/p_/w_` ak47, deagle, awp, m4a1, knife; `sound/weapons/*.wav`).
Plan: read them **at runtime from the install** (like MW2 — never copy into
repo): GoldSrc MDL v10 loader + WAV, weapon rules from ReGameDLL
(damage/range/armor+wall penetration/spread/recoil punchangle/ROF/reload/
per-weapon maxspeed/hitgroup multipliers). Knife first. Starts after the
per-tick cmd + movement + FPS work.

## Step order agreed 2026-10-05 (user)
7 (real CS models/sounds) → 1 (AK headshot not one-tapping: likely CS head
×4 not reaching the damage path — 3 HS = 36×3; perks+deathstreaks off —
spawn HUD shows Sleight of Hand/Stopping Power/Steady Aim; crouch-jump hold:
ask user whether he stands mid-air holding CTRL, SPACE vs wheel) → 4 knife
→ 3 armor → 2 CS penetration → 5 rounds/money/buy → 6 C4 → 8 FPS thread.
Also logged: rare car-explosion particles all over map (couldn't
reproduce; ask for `clip` right after).

## Direction change 2026-10-05: CS:Source models replace the 1.6 ones
User: 1.6 models' lighting looks off → port **CS:S** viewmodels (and later
knife, HE/flash/smoke, C4, sounds) from `X:\SteamLibrary\steamapps\common\
Counter-Strike Source` (found via Steam libraryfolders; read at runtime from
its VPKs, never copied). **Gameplay numbers stay CS 1.6** (spray/recoil,
damage, accuracy); CS:S anims time-scaled to 1.6 reload/draw times.
CS:GO install is probably CS2 (Source 2) — not used.
Agreed list (in order):
1. Sway: user saw too much up/down + saw behind the hand (my 1.6 bob + turn
   lag stacked ~7u). Make it CS:S-like gentle + `cl_bob 0` style off switch.
   Light the gun from the MW2 map (light grid at the player), not constant.
2. AWP scope like CS: instant right-click zoom, 2 levels (fov 40 → 10 →
   off), unzoom after shot + re-zoom after bolt, CS black overlay with thin
   lines — replaces MW2 Intervention ADS (slow zoom-in, one level).
3. G = drop weapon (not knife) + walk-over pickup if slot empty, E swaps;
   G is MW2 frag today → rebind (defaults + user settings.cfg).
4. VPK reader + Source MDL (v44–48: .mdl/.vvd/.vtx, VTF/VMT) loader → CS:S
   guns (v_rif_ak47, v_rif_m4a1, v_snip_awp, v_pist_deagle, v_pist_usp,
   v_pist_glock18).
5. Knife (CS:S model + CS rules: slash 15/20, stab 65, backstab 195).
6. Step 1 fixes (AK headshot ×4 not applied — 3 HS = 36×3; perks +
   deathstreaks off; crouch-jump hold detail still unanswered).
7. CS held grenades on slot 4 (draw, hold attack = pin, release = throw,
   auto back to last gun; G/Q offhand off): HE ~98 dmg/350u, flash blind by
   distance+facing, smoke ~15–18s; buy limits 1/2/1.
8. Armour, CS penetration, rounds/money/buy menu, C4.

## Step 7: CS 1.6 viewmodels from the user's install (2026-10-05, working)
- `crates/mdl_goldsrc` (new): GoldSrc MDL v10 parser (layouts from public
  `studio.h`, own code): bones, sequences (RLE anim channels decoded to
  per-frame bone pos/quat, `AngleQuaternion` convention), slerp posing,
  strip/fan → triangle lists (one bone per vertex/normal), 8-bit textures
  → RGBA (masked index 255). Tests incl. `retail_viewmodels_parse_when_
  installed` (set `IW4L_CSTRIKE` to run): all of v_ak47/m4a1/awp/deagle/
  usp/glock18/knife parse (~1k tris, 40–45 bones). Events NOT parsed yet.
- Install discovery: `asset_transport::find_cstrike()` (`IW4L_CSTRIKE` or
  `<steam lib>/steamapps/common/Half-Life/cstrike`), read at runtime only.
- Render: `render_gpu/src/drawsurf/cs_viewmodel.rs` + `.wgsl` — own pass in
  `Core3d` PostProcess after PostFx, before the 2D HUD (`CsViewmodelSet`;
  `draw_iw_tess` runs after it). Static VB per model, GPU rigid skinning
  (≤128 bones × 3 vec4 in a uniform), view-space projection (GoldSrc axes),
  CS fov 90@4:3 (tan½fovy 0.75, Hor+), reverse-Z own Depth32 cleared per
  frame (gun never clips walls), lit/fullbright/additive pipelines, alpha
  discard for masked. Constant wrapped-lambert lighting (ambient .5 + .6).
- Game side: `render_anim/src/occupancy/cs_viewmodel.rs` —
  `update_cs_viewmodel` (after `PresentedPublished`, before
  `occupy_fpv_scene`): model per `CsWeapon::view_model`, lazy load,
  anims draw/shoot(random of shootN, `_unsil` set for M4/USP)/reload/idle
  loop, gun not punched (inverse `cs_punch`), mirrored right-handed (CS
  `cl_righthand 1`), hidden when AWP scoped/dead/3rd person.
  `CsViewmodelActive` makes `occupy_fpv_scene` skip the MW2 viewmodel.
- Verified by screenshots: AK (draw/idle/shoot with dip), Deagle, Glock,
  M4, USP render right-handed like CS 1.6.
- User test #1 (2026-10-05): "impressive"; Deagle looked too dark → added
  GoldSrc texture gamma (`pow(rgb, 0.8)`, texgamma 2.0/gamma 2.5) and
  camera-side light (ambient .65 + .45, toward-light (-0.6,0.3,0.75)); no
  CS v_ texture has chrome/fullbright flags (checked). Ask if still dark.
- **Sway** (2026-10-05): CS 1.6 `V_CalcBob` (cl_bob .01, bobcycle .8,
  bobup .5; forward ×0.4 + world-up offset, pitch −.3/yaw −.5/roll −1 ×bob)
  + Source-style turn lag (catch-up 5/s, max lag 1.5, 2 units per unit of
  facing lag) in `sway_placement`. Verified bob in screenshots (~60px).
- **Slots** (2026-10-05): input commands `slot1`–`slot5` (ids 78–82 appended
  to `INPUT_COMMAND_NAMES`, now 83) → `ClientInput::weapon_slots` → client
  `select.index` = owned weapon with `weapon_iw4::cs::slot_of` (CS slot,
  else MW2 pistol 2 / primary 1). Binds 1–5 = slot1–5 in
  `default_controls.cfg` AND the user's `iw4l-artifacts/settings.cfg`
  (was `2 +left`, `3 +right` = "look around"; backup
  `settings.cfg.bak-before-slots`). `buy` of a CS gun replaces the owned gun
  in the same slot (else adds) — `apply_give_weapon` `outgoing`; verified
  AK + Deagle both owned.
- **CS sounds** (2026-10-05): `asset_audio/src/cs_sounds.rs` —
  `append_cs_weapon_sounds` (called when the match bank is composed,
  `audio/src/ambient.rs`) decodes every `cstrike/sound/weapons/*.wav`
  (8/16-bit PCM → 16-bit) into aliases `cs/weapons/<name>` (3D, modelled on
  `weap_ak47_fire_npc`) and `cs/weapons/<name>/plr` (2D, on
  `weap_ak47_fire_plr`) via new `SoundCatalog::add_loose_alias`. 165 loaded.
  Fire: `CsWeapon::fire_sounds` (ak47-1/2, m4a1_unsil-1/2, awp1, deagle-1/2,
  usp_unsil-1, glock18-1/2), picked by shot correlation, swapped in at the
  fire-alias choice in `render_frontend/src/adapters/fx/system.rs`.
  Reload/draw/bolt: MDL events (now parsed: `Sequence::events`, 5004 =
  sound `weapons/x.wav`, 5001 = muzzle flash) played from
  `update_cs_viewmodel` as the anim passes their frame; MW2 viewmodel
  notetracks muted for CS guns (`publish_fpv_notetracks`). Not yet heard by
  the user. ("sound bank ready in ~150 s" log is pre-existing, all runs.)
- Next in step 7: muzzle flash on the CS model (event 5001; MW2 flash is
  still bolted to the hidden MW2 gun's tag), world lighting for the gun,
  p_/w_ models; knife model comes with step 4.

## CS weapons stage 1 (2026-10-05): CS 1.6 rules on MW2 twin guns
Stage 1 = MW2 model/anims/sounds, CS 1.6 behaviour (numbers read from
ReGameDLL `wpn_*.cpp`/`weapons.h`, **non-REGAMEDLL_FIXES/ADD paths = retail
Valve 1.6**). Stage 2 = real CS models/sounds from the install (MDL loader).
- Table + formulas: `crates/weapon_iw4/src/cs.rs` (`CS_WEAPONS`):
  ak47→`ak47_mp`, m4a1→`m4_mp` (unsilenced), awp→`cheytac_mp`,
  deagle→`deserteagle_mp`, usp→`usp_mp` (unsilenced), glock→`glock_mp`
  (semi). 10 unit tests (`cargo test -p weapon_iw4 --lib cs::`).
- Accuracy: rifles `shots³/divisor + base` with **integer division**
  (retail quirk: AK first 5 bullets acc 0.35, 6th → 1.25), initial 0.2,
  spread uses the accuracy *before* the shot; pistols lose accuracy when
  spammed (`(recover - dt) * factor`). Reload/switch reset the spray.
  `post_frame`: release trigger → shots capped 15, 0.4s pause, then −1 per
  22.5ms (strict `<`, = every 30ms at 100Hz); pistols reset to 0.
- Recoil: retail `KickBack` (rifles) / flat −2 pitch punch (deagle, usp,
  awp; glock none) into `ps.cs_punch`; side flip hashed from (client,
  server_time, shots) so prediction agrees. Punch decays per command
  (`drop_punch`, (10 + len/2) deg/s). Bullets fire along view + punch;
  MW2 `gun_angle_offset` (sway) ignored for CS guns.
- Spread: `FireBullets3` triangular (`cs::bullet_direction`, rng from the
  shot's combat seed), in `AcceptedShot::cs_spread`.
- Damage: `damage * range_mod^(dist/500)`, max distance 8192/4096
  (`sim/src/bullet.rs`); hitgroups head ×4, stomach ×1.25, legs ×0.75
  (`CS_LOCATION_DAMAGE`). No CS wall penetration/armor yet (MW2 pen still).
- Facts override: `session/src/combat_table.rs` `apply_cs_rules` (runs on
  every peer before `set_weapon_combat_table` → content digest includes
  `cs_weapon`). Cycle/clip/reserve/reload/fire_type/no ADS for non-snipers;
  bolt AWP: fire_time = 1450 − MW2 rechamber time. Move scales = maxspeed/250
  (AK 221, M4 230, AWP 210 / 150 scoped); Source/GoldSrc pmove scale
  `max_speed` by `rules::weapon_speed_scale`.
- PlayerState: `cs_punch, cs_shots_fired, cs_accuracy, cs_last_fire_ms,
  cs_recoil_dir, cs_decrease_shots_ms, cs_delay_fire, cs_gun_weapon`
  (netfields Exact). Sim hooks in `sim/src/combat.rs`
  (`cs_weapon_frame`, `cs_weapon_fire`, `cs_bullet_direction`).
- Client: camera = view + punch (`net/src/client/runtime.rs`
  `publish_presented`); MW2 view kick + sway off for CS guns
  (`render_anim/src/occupancy/view_kick.rs`).
- Console: `buy <ak47|m4a1|awp|deagle|usp|glock>` = GiveWeapon of the twin
  (free; still needs debug actions like `give`). Replaces the held gun.
- Verified 2026-10-05 (scripted, mp_boneyard): log `CS 1.6 rules on:
  ak47_mp cheytac_mp deserteagle_mp glock_mp m4_mp usp_mp`; AK drawn acc
  0.2 → ~1s spray 11 shots, acc 1.25, punch (−5.0, +0.99) → 1s after
  release shots 0, punch 0; deagle tap fires, acc 0.9. Client presented
  CS fields bit-identical to authority; 20s window dev=2 (legs_anim only),
  forced=0. Awaiting the user's hands-on test (feel, hit damage on bots).
  Retail quirk kept: accuracy is only recomputed on the next shot, so the
  first tap after a full spray still uses 1.25 (FIXES builds reset it).
- User play-test 2026-10-05: AK spray good; `mv_stamina 1` (default) is
  right — keep. Possible long-range hit issue → retest with CS crosshair
  (MW2 reticle drifted off-centre with sway while CS bullets go dead centre
  + punch, a likely cause).
- **CS crosshair** (2026-10-05): `hud/src/cs_crosshair.rs` — 4 green bars
  (50,250,50) fixed at screen centre, sized in 640x480 units ×
  height/480. Gap per weapon (`CsWeapon::crosshair`: AK 4/+4, M4 4/+3,
  pistols 8/+3, knife/empty `CS_KNIFE_CROSSHAIR` 7/+3), ×2 air, ×0.5
  crouched, ×1.5 above 140 u/s; each shot (`cs_last_fire_ms` change) +delta
  (cap 15), shrinks −(0.1 + 1.3%) per 10ms. Snipers: none (scope overlay
  when zoomed). Hidden when ADS/dead/killcam. MW2 reticle hidden whenever
  the CS one applies. Console `cl_dynamiccrosshair 0|1` (not persisted).
  Formulas from CS 1.6 behaviour / Momentum `hud_crosshair.cpp` (read only).
- **Player boosting** (2026-10-05): under CS rules other players are
  solid **boxes** (±15, height = their stand 72 / duck 54 hull,
  `rules::body_height`) instead of IW4 capsules, swept with an AABB slab
  test (`sim/src/step.rs` `trace_box_through_body`) → flat heads, you can
  stand on players (ground entity = their client num). Verified: dummy bot
  dropped on the local player rests on the head (Δz 74). Remote bodies in
  client prediction carry `box_height` too. A crouched booster can't stand
  up while someone is on them (unduck trace hits the body). No carry when
  the booster walks (as CS).
- **Duck-jump stuck fix** (2026-10-05, user report: got stuck in objects
  after duck-jumps, had to crouch + back out). Cause found: IW4 sweeps
  only meet surfaces they move toward, so `can_unduck`'s in-air downward
  sweep never saw walls/overhangs the taller hull poked into. Now also a
  zero-length position test at the stand spot. Safety net `Move::unstick`
  (Source `CheckStuck`/`FixPlayerCrouchStuck`): a command starting inside
  the world (bodies ignored) crouches if the duck hull fits, else nudges
  ≤2u (`STUCK_NUDGES`); authority logs `movement: client N started a
  command inside geometry at [...]` — ask for those lines if it recurs.
  Stress run with duck-jumps: 0 warnings, prediction forced=0.
- **Fixes after play-test #2 (2026-10-05)** — user: "movement still broken,
  crouch is toggle, weapons very low range, deagle/jump accuracy not 1.6":
  1. *Real stuck cause found* (`source.rs` `try_player_move`): airborne +
     pushing into an IW4 **mesh** wall → mesh sweeps report a fraction-0
     hit for a glancing/parallel move inside the 0.125 clip epsilon → my
     port treated the repeat plane as a crease → velocity zeroed every tick
     → player hung mid-air (reproduced: (249,1597) 53u off the floor, vel
     (0,0,-4)). Fix = IW4/Q3 `PM_SlideMove` rule: same plane again (dot >
     0.99) → push velocity out along its normal. Test
     `pushing_into_a_mesh_wall_in_the_air_still_falls` (`MeshWall` backend
     mimics the mesh epsilon). 12-direction duck-jump session: 9 hangs → 0.
     Diagnostic kept: authority warns `movement: client N hanging in the
     air at [...]` + trace probes (`report_edge_hang`, `sim/src/step.rs`).
  2. *Crouch toggle*: user's `settings.cfg` has `bind CTRL togglecrouch` /
     `toggleprone` (MW2 menu binds, override default_controls). Under CS
     rules `ClientInput::hold_crouch` (set in `net/src/client/input.rs`)
     maps +stance/+prone/togglecrouch/toggleprone/goprone/gocrouch →
     held `+movedown` (`input_iw4` `effective_binding`, 2 tests).
  3. *Range*: measured — no range cap (4096/8192 travel), lag-comp Exact,
     damage = CS math. Real issues fixed: (a) **MW2 health regen** undid
     long-range damage → `scr_player_healthregentime 0` pushed into the
     match script dvars after `mp/…cfg` (`session/src/match_apply.rs`;
     the cfg sets 5); verified health 60 stays 60. (b) MW2 camera sway/bob
     (idle, ADS, scope) still moved the camera while CS bullets go dead
     centre → off for CS guns (`render_anim/.../view_kick.rs`), and
     `ViewweaponAim` angle offset + xhair zeroed for CS guns
     (`fpv_present.rs`) so the AWP scope overlay is centred. (c) hold-breath
     off for CS guns (+ hint hidden). Note: MW2 bullet penetration through
     thin props still scales damage (seen ×0.88/×0.264); CS penetration
     rules not ported yet.
  4. Deagle/jump accuracy: code matches retail 1.6 (air 1.5×(1−acc) etc.);
     asked user what exactly differs.
  - Debug aids: `IW4L_GSC_DUMP=<dir>` writes all packaged GSC sources as
    they load (`assets/src/script_sources.rs`); per-shot `cs shot:` logs at
    debug level (`sim/src/combat.rs`).
- Not yet: knife, armor/kevlar+helmet, CS penetration, M4/USP silencer,
  Glock burst, AWP 2-level zoom (MW2 scope), CS crosshair, money/buy menu,
  MW2 health regen still on (GSC).

## Performance baseline (2026-10-04, same bench script, user's PC)
`IW4L_BENCH=1`, mp_boneyard, 3 bots, 15s, AutoNoVsync.
- **Original IW4L (d352dbd, 20Hz, worktree `context/mrs/baseline`):
  107 fps avg, p50 129.** Update 6.4ms (Present 5.1), Step 6.5ms/tick ×0.19
  = 1.2ms/frame, render thread 6.2ms (parallel).
- **Ours (100Hz + scheduler gate): 58 fps.** Update 7.8, sim+net ~3.9ms/tick
  ×1.73 = ~7.6ms/frame.
- Listen server: authority runs as FixedUpdate systems on the main thread,
  sharing ~25 resources with client systems (`net/src/authority/runtime.rs`
  `register_listen_runtime`). 100 ticks × ~3.9ms = ~390ms/s of main thread
  regardless of fps. Moving authority to its own thread = big refactor
  (later candidate). `iw4l serve <zone>` dedicated exists but clients join
  via master relay + TLS certs (not a quick local path).
- 200fps needs client Update ≈3ms and much cheaper ticks. Even original
  IW4L can't hit 200 on this PC (client side alone 6.4ms).
- Client sends **one usercmd per render frame with variable msec**
  (`predict_local_move`, `ClientPrediction::predict`) → CS/Source movement
  integrates differently at different fps, hitches = one giant step. Fix in
  progress: one cmd per 10ms tick + local-player interpolation + live look
  angles (Source model).
- Fixed: `MAX_UNACKED_SNAPSHOTS` was 20 snapshots (=200ms at 100Hz → forced
  prediction resync on any hitch) → `ticks_for_ms(1000)`.

## Profiling tools (Windows)
- `IW4L_BENCH=1` run → `target/play/iw4l-artifacts/bench/*.txt` (span tree,
  counters). Perfetto (`IW4L_PERF`) is unavailable on Windows builds.
- **Per-system CPU profile**: build with
  `CARGO_TARGET_DIR=target-trace cargo build --profile play -p launcher --features bevy-profile`,
  run with `RUST_LOG=warn,iw4l=info,bevy_ecs::system=info`; every 10s the
  log gets `system profile:` rows (top systems, ms/frame).
  (`bevy-trace` chrome traces are useless here: thousands of systems/frame
  → 6 GB file, writer falls behind and loses gameplay.)
- Scripted runs: `--cmds 'wait world; spawn 0; wait ambient; force_match_start; ...'`,
  `dump <name>` writes full state (stalls ~1s), `hold/press/release +input`,
  `mouserate`, `look <yaw> <pitch>` (waits for pose; don't use while moving).
  Note `spawn 0` with no other players always picks the same cramped spawn.

## State at end of 2026-10-04 autonomous session (nothing committed)
- Final stress (mp_boneyard, 3 bots, 75s bhop spam): 83 fps avg, max frame
  58ms (no freezes), prediction dev=0 forced=0. 24 movement tests pass.
- Leftovers on disk: worktree `context/mrs/baseline` (original code, for
  A/B benches — remove with `git worktree remove context/mrs/baseline`),
  `target-trace/` (profiling build, several GB, deletable).
- For the user to test: CS:GO movement feel (bhop with MWHEELDOWN/SPACE,
  air strafe, duck-jump onto clutter, crouch, walk SHIFT, CS ladders),
  freezes gone, fps. Existing configs may need `binddefaults`.
- Next candidates: authority world on its own thread (biggest fps win),
  Arc-shared snapshots in `fanout_loopback`, trimming the prediction
  world's per-command schedule, client Present systems; then CS 1.6
  weapons from the user's install (MDL loader + ReGameDLL weapon rules).

## Switching movement style
One line in `crates/movement_iw4/src/rules.rs`:
`pub const ACTIVE: Option<Ruleset> = Some(Ruleset::Source(source::CSGO));`
- `Ruleset::Source(source::CSGO)` — CS:GO competitive (default)
- `Ruleset::Source(source::MOMENTUM_BHOP)` / `MOMENTUM_SURF` — Momentum modes
- `Ruleset::GoldSrc(cs::CS16)` — CS 1.6
- `None` — original MW2 movement
Client and server must run the same build (prediction uses the same constant).
Tunables (speeds, accel, hull, stamina...) are the profile struct fields.

## Session log 2026-10-04 (autonomous, user away)
1. **Per-tick usercmds (Source model)** — `net/src/client/runtime.rs`
   `predict_local_move`: one cmd per 10ms tick aligned to the authority
   grid (catch-up ≤ `MAX_CMDS_PER_FRAME`=8, restart if clock steps back
   >1s); edges consumed only when a cmd is sent. `ClientPrediction`
   keeps `interp_from`/`interp_time`; `presented_local()` blends origin/
   velocity/eye between ticks; `publish_presented` applies live `LookState`
   angles every frame (pm_type 0). Prediction counters logged every 20s
   (`net info prediction: ...`). Result: 58→74 fps, dev=0 forced=0.
2. **Freeze fixes** — (a) `diag` logger now writes on its own thread
   (`AsyncFile`, BufWriter, flush per batch; Error level and `diag::flush()`
   wait). Root cause: synchronous per-line file write+flush under a global
   mutex stalled 145–245ms (Windows/AV) inside `smodel buckets` info logs
   (5k lines/90s); that line is now `debug`. Permanent `slow static draw
   rebuild` warn (≥20ms) in `render_frontend/.../retained_list.rs`.
   (b) settings autosave writes on a thread (`console/src/user_settings.rs`,
   joined on exit). Stress (75s bhop spam): max frame 300–709ms → ≤72ms.
3. **Source / Momentum movement** — `movement_iw4/src/source.rs` +
   `rules.rs` (`ACTIVE = Source(CSGO)`; GoldSrc(CS16) still available).
   Ported from Momentum `mom_gamemovement.cpp` (CS-based modes): FullWalk
   order, StartGravity/FinishGravity, CheckJumpButton (+impulse, stamina,
   jump z-offset 1.5), Friction, Accelerate/AirAccelerate (cap 30),
   WalkMove + classic StepMove + StayOnGround, TryPlayerMove (overbounce 1,
   crease, simplified ramp fix), CategorizePosition (non-jump vel 140,
   quadrant probe, landing clip + slope fix), Duck/Unduck (0.4/0.2s, air
   shift 0.5×hull gap). `CSGO` profile: maxspeed 250, keys 450, walk .52,
   duck .34, accel 5.5, airaccel 12, friction 5.2, stopspeed 80, jump
   301.99 (57u), CS:S stamina (Momentum KZ: 100/25/19, ref 1/70), hull
   72/54, eye 64.06/46.04, probe 2, fall 580/1024×1.25. Presets
   `MOMENTUM_BHOP` (autohop, airaccel 1000, maxspeed 260) and
   `MOMENTUM_SURF` (airaccel 150). 23 movement unit tests pass. In-game:
   eye 64.06/46.04 verified, dev=0 forced=0, 91 fps no bots.
   Momentum repo: `context/externals/momentum` (Source 1 SDK License —
   reference only, no copied code).
5. **More frame-time cuts** (bench mp_boneyard, 3 bots: 74 → **86 fps**;
   92 with `sm_enable 0`; original IW4L 107 at 20Hz):
   - GSC heap GC once per second, not every script frame
     (`HEAP_COLLECT_PERIOD_MS`, `sim/src/script/runtime/mod.rs`).
   - Bots think at 20 Hz (`BOT_THINK_MS`, `bots/src/plugin.rs`) and resend
     `BotSlot::last_cmd` on in-between ticks (they still move at 100Hz).
     Was a full world snapshot + traces every 10ms tick.
   - `console::local_account::save`: per-frame full persistent-data copy
     replaced by a revision compare (`PersistentData::revision`); the
     account file write moved to a thread (joined on exit).
   - Per-system profile (bevy-profile build) top main-thread items now:
     sim step ~2.6ms/tick (+prediction re-runs the whole sim schedule per
     predicted cmd), `fanout_loopback` 0.45ms/tick (deep-clones the full
     snapshot several times per tick — candidate: Arc snapshots),
     client Present ~4.5ms (script_model posing 0.43, remote bodies 0.23,
     fpv skin 0.23, receive 0.46, reconcile 0.44, scorebar 0.21...).
   - `fanout_loopback`: no throwaway full-snapshot clone before building the
     viewer snapshot (Fanout 0.45 → 0.38ms/tick). The loopback deliberately
     encodes/decodes like a remote link (parity), which is the rest of
     Fanout + client Receive. Latest bench: 3 bots **94 fps**.
   - **Biggest remaining structural win: run the listen server's authority
     world on its own thread** (~3.5ms/frame off the main thread). Large
     refactor (shared resources), not started.
4. **CS/Source ladders** — `source.rs` `ladder_move`: IW4 still detects and
   attaches (`check_ladder_move`, SURF_LADDER, `v_ladder_vec` = outward
   normal); movement is HL/Source `LadderMove`: climb 200 u/s along the
   view (forward into the ladder climbs, look down + forward descends),
   strafe lateral, duck ×0.34, no gravity, jump = push off at 270 along the
   normal (+ `jump_time` so IW4's 300ms re-attach block applies), walking
   away on the floor steps off. Unit test with a two-plane ladder world.

## Direction change (2026-10-04, after user play-test)
**Movement target is now Momentum Mod (CS:GO/CS:S Source movement), not
CS 1.6.** Reference: github.com/momentum-mod/game (read, reimplement, never
copy — check its license). Default = competitive CS:GO rules (stamina,
landing slowdown, bhop limits, no autohop); bhop/surf server presets later.
- **Player size: CS:GO** (taller duck hull/higher jump fit MW2 clutter that
  CS 1.6 size couldn't clear). Take exact hull/eye/jump numbers from Source.
- **Tick: keep 100Hz** (smoother bhop + hitreg; at 200fps the sim is only
  ~0.5 tick/frame ≈1.4ms). Only drop to 64 if profiling proves the tick is
  the FPS limit (64 needs a time-model rewrite: integer ms).
- The CS 1.6 profile (`cs.rs`, `CS16`) stays as a second profile.
- **User play-test feedback on the 1.6 build:** duck-jump onto boxes great;
  MWHEELDOWN jump sometimes lost (suspect `was_pressed` latch cleared on
  frames that build no 10ms cmd slot); jump/duck-jump height "a bit off"
  and also with SPACE (check stamina strength + client-prediction
  corrections); **freezes when spamming bhop** (unprofiled); FPS < 100.
- **Performance goal: 200+ fps** on user's PC (RTX 3060, Ryzen 5 5600G,
  16GB) — CPU/main-thread bound (~16ms). Suspects: per-frame
  "render frame diag" log line, client Present ~6ms, shadows ~3ms (make
  optional), snapshot Ingress/Fanout ~2ms/frame. Milestone 144, then 200.
- Later (after movement+perf testing): **CS ladders** (Source/GoldSrc
  ladder rules, not CoD climb), **C4 bomb**, **buy menu**, then
  health/armor/hitboxes, knife, weapons.
- Order: (1) freezes + lost-jump fixes, (2) Momentum/CS:GO movement
  profile, (3) FPS pass toward 200, (4) CS ladders.

## CS-fork changes applied
- 2026-10-04: **no sprint, no prone** — `pmove` strips the SPRINT and PRONE
  buttons from every usercmd (`crates/movement_iw4/src/single.rs`). Crouch
  is already hold-to-crouch, like CS.
- 2026-10-04: **no MW2 weapons/equipment/killstreaks** — GSC `giveweapon` is
  a no-op via `SCRIPT_GIVES_NO_WEAPONS` (`crates/sim/src/script_player.rs`).
  Players and bots spawn empty-handed. The console `give` command still
  works. Replacement weapons will come from CS assets (user mentioned
  Xash3D/CS 1.6 models — GoldSrc `.mdl`, convert the same way as sub-project 3).
- 2026-10-04: **Phase 3 — 100Hz tick (done, user-tested OK: timer normal,
  no rubber-banding).** FPS 40–80, ~200 looking at sky → render-bound, not
  the 100Hz sim; check whether it predates the tick change.
  `sim::MATCH_TICK_MS = 10`; net `AUTHORITY_MS`/`AUTHORITY_HZ` and replay
  `CLIP_TICK_MS` now derive from it. 20Hz-tuned tick counts rewritten as
  durations via `sim::ticks_for_ms(ms)` (bots, audio, net archive/proxy/
  resync, lag-comp history/rewind, mechanics settle). Fixed literal `/ 50`
  in `sim/src/item.rs` and `bots/src/sensor.rs`; javelin per-tick speed
  bleed compounded by time; bot escape `+16` ticks → 800ms. Removed the
  stale `MATCH_TICK_MS == 50` assert in `sim/src/damage.rs`. Killcam's
  `SERVER_FRAME_MS = 50` left alone (script-frame semantics). Old demos
  won't replay; netcode sends 5x snapshots (Phase 4 watch item).
- 2026-10-04: **CS 1.6 movement (code done, 11 unit tests pass, awaiting
  in-game test).** `crates/movement_iw4/src/cs.rs`: `MovementProfile` +
  `CS16` const + `ACTIVE_PROFILE` (`None` = retail IW4). `single::pmove`
  dispatches to `cs::pmove` for `pm_type == 0`. Implements GoldSrc
  `pm_shared` rules (read from ReGameDLL): accelerate 5, airaccelerate 10,
  30 u/s air wish cap, friction 4 + edge friction 2 + stopspeed 75, jump
  45u (`sqrt(2*800*45)`), bhop cap 1.2×maxspeed → ×0.8, jump stamina
  1315.79ms (`fuser2`: lowers next jump + ground speed), maxspeed 250,
  walk ×0.52 (SHIFT/sprint button = walk), duck ×0.333, 400ms ground duck,
  instant in-air duck lifting feet 18 (duck-jump), double-duck hop,
  2-unit ground snap, no velocity snapping, no MW2 landing slowdown
  (guarded in `crash.rs`), no mantle, no 500ms jump cooldown.
  Hull 72 stand / 36 duck, eye 53 / 30 (CS `VEC_VIEW 17`, `VEC_DUCK_VIEW 12`).
  New replicated PlayerState fields: `cs_stamina`, `cs_duck_time`,
  `cs_duck_state`, `cs_fall_velocity` (netfields.rs). Ladders still use
  IW4 ladder code. Default binds: CTRL = duck, SHIFT = walk,
  MWHEELDOWN = jump (`console/assets/default_controls.cfg`; existing
  configs need `binddefaults` or manual binds).
  13 unit tests in `cs.rs` (`cargo test -p movement_iw4 --lib cs::`):
  250 run / 130 walk / 83 duck, diagonal capped, friction stop ~0.5s,
  45u jump + no pogo, stamina lowers back-to-back jumps, bhop cap → 240,
  air-strafe gains speed, duck-jump lift, double-duck hop, ground duck,
  60° ramp surf keeps speed, fall damage table. In-game (scripted dumps):
  eye 53/30, duck state, jump + stamina, landing, friction stop verified.
  CS fall damage done: `cs::fall_damage` (ReGameDLL: safe 500, fatal
  1100, ×1.25 → 1100 u/s = 125 dmg), reported via
  `PmoveResult::landing_speed`, applied as MOD_FALLING world hit in
  `sim/src/step.rs` → `script_player::fall_damage` (authority only).
  Next movement work: per-weapon maxspeed when weapons exist, ladder →
  GoldSrc ladder rules.
- 2026-10-04: **FPS fix for 100Hz.** Bench (`IW4L_BENCH=1`, mp_boneyard,
  3 bots) showed the sim `Step` at 5.3 ms/tick × 2.7 ticks/frame = 53% of
  the frame (37 fps). Per-system probe: GSC `advance_scheduler` was
  3.2 ms/tick, running every 10ms tick though waits land on 50ms frames.
  Now it skips non-frame ticks unless threads were just spawned (or it
  has never run — skipping the first run broke `codecallback_playerconnect`
  before init). Result: Step 2.8 ms/tick, **58 fps avg (was 37), p99 29ms
  (was 90)**. Remaining per tick: sync_players+presence 0.55, run_players
  0.6, entity types 0.28, net Ingress+Fanout ~1.1. Frame is now ~half
  client Present/render (not tick-related). 100Hz kept.
- Known issue (pre-existing, not from our changes): bots stopped deciding
  ~5 min into a match on mp_boneyard (log: connector/execution attempts
  drop to 0 after tick ~5951). Investigate later.

## Working agreements
- Keep this file current: when a phase starts, finishes, or a finding/decision
  changes the plan, update the relevant section (with the date) in the same
  session — it's the only memory that carries between chats.
- Ask-permission mode default; this is a personal fork, not upstream iw4L —
  commits should go to a fork/private repo, not a PR to vladtrc/iw4L.
- Validate each phase (especially tick-rate step 2, the script-clock
  decouple) before moving to the next; don't stack unverified changes.
- Docs worth re-reading as work progresses: docs/SIM-STEP.md,
  docs/GSC-RUNTIME.md, docs/MAP-LOAD.md, docs/RUN.md.

## CS:S viewmodels in game (2026-10-05, autonomous)
- `render_anim/src/occupancy/cs_viewmodel.rs` rewritten for both formats: CS:S
  (`mdl_source`, VPK from `find_css_pak`) preferred, CS 1.6 fallback. Roles from
  activities (`ACT_VM_IDLE/DRAW/RELOAD/PRIMARYATTACK`; plain set = unsilenced M4/USP).
  Reload anim time-scaled to the 1.6 reload time. **Both formats are left-handed
  and mirrored** (CS:S scripts `BuiltRightHanded 0`). CS:S fov: viewmodel_fov 54.
  Verified screenshots: AK, M4, AWP, Deagle (silver), USP, Glock all right-handed.
- **Colour fix**: main target is `Rgba8Unorm` (gamma values); textures sample as sRGB,
  so the shader now encodes lit colour back to sRGB (`encode()` in
  `cs_viewmodel.wgsl`). This was the "orange skin / too-dark deagle" cause.
  Non-alphatest Source materials get alpha forced to 255 (alpha holds masks).
- Sounds: fire alias = `css/<css_fire_sound lowercase>[/plr]`, falls back to 1.6.
  Verified starts: `css/weapon_ak47.single/plr`, `.clipout`, `.clipin` (134 CS:S
  entries in bank). Scripted `press` holds 150ms → 2 AK shots; not a bug.
- Sway: gentle CS:S-style bob/turn lag; console `cl_wpn_sway 0|1`.
- **Map lighting** (`render_frontend/src/adapters/anim/cs_viewmodel_light.rs`):
  light grid at `viewmodel_lighting_origin` → ambient ×2.4 + camera-side fill ×3.2
  (linear of sRGB grid rgb); sun = map sun colour ×0.55 × visibility, visibility from
  a CONTENTS_SOLID trace toward the sun (sky flag 0x4 or no hit), faded 6/s.
  Verified darker in shaded corner / natural in warehouse like MW2's own gun.
  **Not yet verified: a spot with clear sky** (all tested spots were shaded or under
  grates; grates block the trace). Test with `move <x> <y> <z>` (needs cheats) in
  the open + `IW4L_LOG_LEVEL=debug` → `cs viewmodel light: sun visible`.
- Leftover cosmetic: HUD shows MW2 weapon names ("Intervention").
- Next (agreed order): AWP scope (instant 2-level zoom, CS overlay), G drop/pickup,
  knife, step-1 fixes (AK HS ×4, perks off, crouch-jump), grenades, armour/etc.

## Viewmodel FOV + CS knife (2026-10-05, user: "weapons too close", "knife on 3, 250 speed")
- `viewmodel_fov` setting (frame `GameSettings::viewmodel_fov`, saved in settings.cfg,
  console `viewmodel_fov [54-90]`, default 68; was HL2's 54 = too close). Source-style:
  horizontal at 4:3, Hor+ widened. Momentum default 65, CS:S retail reportedly 74.
- **Knife** (`weapon_iw4::cs::CS_KNIFE`, index `CS_KNIFE_INDEX` = 7, outside `CS_WEAPONS`
  so `cs_weapon()` stays guns-only; `cs_weapon_index_for("beretta_mp")` → knife). Worn
  by MW2 `beretta_mp` (never fires). Retail numbers (ReGameDLL non-FIXES): slash 15 /
  48u (next 0.4 hit, 0.35 miss; stab after 0.5), stab 65 / 32u (1.1 hit, 1.0 miss),
  backstab (2D yaw dot > 0.8) ×3 = 195; line first, then `head_hull` (±16, ±18) fan via
  MW2 `MELEE_TRACE_OFFSETS`; hitgroup ×CS table only for the centre line. Slot 3,
  speed 250. Retail quirk: the 20-dmg "fast slash" never triggers (next-attack is set
  before the check), so slashes are always 15.
- Sim (`sim/src/combat.rs`): `CS_KNIFE_BUTTONS` (attack, ADS, melee, reload) stripped
  from the MW2 weapon machine while the knife is held; `cs_knife_frame` per command
  (Ready state only): stab button (ADS = right click) wins, `cs_knife_trace`
  (lag-comp), authority applies `DamageSource::Melee` with CS amount + MELEE_BLOOD.
  CS guns lose MW2 gun-melee (MELEE_CHARGE stripped).
  New PlayerState (netfields Exact): `cs_next_attack_ms`, `cs_next_attack2_ms`,
  `cs_knife` (bits: anim 1/2 slash, 3 stab, 4 stab_miss; hit nothing/player/world;
  swing count) — `playerstate_iw4::cs_knife`.
- Spawn with the knife (`script_player::give_cs_knife`, CS fork mode only): given at
  spawn and after `takeallweapons`, held if nothing else; `takeweapon` can't remove it;
  `setspawnweapon` ignores unowned (script class) guns.
- Viewmodel: `ViewWeapon` (gun or knife); knife roles by label (`midslash1/2`, `stab`,
  `stab_miss`, same in CS:S and 1.6); **CS:S knife is modelled right-handed (no
  mirror)**, guns are mirrored. Sounds: `css/weapon_knife.{slash,hit,hitwall,stab,
  deploy}` (1.6: knife_slash1/hit1/hitwall1/stab/deploy1), local only.
- HUD: CS crosshair with knife gap (7/+3). Default bind MOUSE2 → `+speed_throw` (hold;
  toggle ADS broke stab/scope).
- Verified (scripted, dummy bot facing then turned): slash 18 (stomach ×1.25), 15,
  stab 65, backstab 195 → kill (`beretta_mp;195;MOD_MELEE`). `press slot3`/`slot1`
  switch knife ↔ AK. Spawn holds the knife.
- Not yet: remote players see the MW2 M9 in hand and hear no knife sounds; HUD shows
  "M9 30" (names/ammo cosmetic, with "Intervention").

## AWP scope like CS (2026-10-05)
- Rules (ReGameDLL `wpn_awp.cpp` + `ItemPostFrame`): right click (ADS button, hold)
  steps zoom 90 → 40 → 10 → 90 every 0.3 s (`CS_ZOOM_DELAY_MS`), 1 s after draw
  (`CS_DEPLOY_ZOOM_DELAY_MS`); a shot while zoomed drops to 90 and remembers the level
  (`cs_last_zoom`), restored when the MW2 weapon state is Ready again (= shot + 1450 ms);
  reload / switch unzoom; zoomed run speed 150 (verified velocity 150). Unzoomed spread
  +0.08 (already in `CsSpread::Sniper`).
- `CsWeapon::zoom` (AWP `[40, 10]`), `cs::next_zoom`; **MW2 ADS now off for every CS
  gun** (`aim_down_sight = false`). PlayerState `cs_zoom`, `cs_last_zoom` (netfields
  Exact). Sim `cs_zoom_frame` in `cs_weapon_frame`; `cs_weapon_fire` unzooms; spread
  zoomed = `cs_zoom != 0`; `rules::weapon_speed_scale` treats `cs_zoom` as zoomed.
- Camera: `view_kick.rs` `apply_fpv_lens_fov` uses `cs_zoom` as the 4:3 horizontal fov
  (also scales mouse via MW2 `zoom_sensitivity` ≈ CS's 1.2×fov/90). Viewmodel hidden.
- Overlay: GPU pass `render_gpu/src/drawsurf/cs_scope.rs/.wgsl` (fullscreen triangle,
  after the CS viewmodel, **before the MW2 HUD tess pass**, so ammo/minimap stay on
  top): CS:S `sprites/scope_arc` sampled at |pixel−centre|/radius (= the 4 flipped
  quads), black beyond, 1/480-height black lines, CS:S `overlays/scope_lens` grime at
  50 %. Ring inset h/16. Main world: `render_anim/src/occupancy/cs_scope.rs` (loads the
  textures from the pack, generated circle fallback). Zoom click `css/default.zoom`
  (`cs_zoom_sound`, not on shot-unzoom / bolt-resume).
- Verified (scripted): zoom 40/10 screenshots, unzoom on fire, resume at fire+1450,
  reload unzooms, HUD drawn over the scope.

## G drop + pickup (2026-10-05)
- Input command `drop` (id 83, `INPUT_COMMAND_NAMES` now 84) → `ClientInput::drop_weapon`
  → `sim::ClientAction::DropWeapon` (wire tag 23, not debug-gated) →
  `item::drop_cs_weapon`: held CS gun thrown at 400 u/s along body facing (view pitch / 3,
  `DropPlayerItem`), ammo kept, best remaining weapon raised (rifle > pistol > knife).
  Knife can't be dropped. Bind: `G drop` (default_controls + user settings.cfg; backup
  `settings.cfg.bak-before-drop`; G was MW2 `+smoke`).
- Walk-over (`try_touch_one`): a **landed** CS gun is taken when its CS slot is empty (CS:
  weapon boxes only touch on the ground — dropping into a wall re-grabs, as in 1.6).
- Use/E (`grab_number`): a CS gun swaps the gun in **its own slot** (dropped where the new one
  lay); raised only when it ranks at or above the held one (`FShouldSwitchWeapon`).
  MW2 "Press E to swap for AK-47" hint works.
- Verified (scripted): drop AK → Deagle raised; walk over → AK picked + raised; drop AK,
  buy M4, E on the AK → M4 on the ground, AK held.
- Not yet: `buy` replacing a gun doesn't throw the old one (CS drops it); world models are
  the MW2 twins.

## Step 1 fixes (2026-10-05): AK one-tap, perks off
- **Headshots were "none"**: the attached head model's bones (face/jaw/brows, indices
  after the body's) have part classification 0 and sit in front of the body's head box
  (class 2), so bullets hit a class-0 bone → hitloc none → no ×4 (user's "3 HS").
  Fix: `world.rs` `player_hitvol_bones` wraps `_raw` and reclassifies class-0 bones of the
  head model (bone ≥ body bone count) as `HITLOC_HEAD`. Verified: one AK headshot at 300u
  = 140 (`MOD_HEAD_SHOT;head`) → kill. (Probably also an upstream MW2 headshot bug.)
- **Stopping Power ×1.4** came from MW2 GSC (`self.perks[]` + `cac_modified_damage`), not
  the engine. Fixes: GSC `setperk` is a no-op in the CS fork
  (`script_player::SCRIPT_GIVES_NO_PERKS`; kills perk/deathstreak effects that check
  `hasperk`), and `finishplayerdamage` uses the engine's own hit amount for CS weapons
  (any MW2 script modifier ignored). Verified: 2 stomach hits at 300u leave 14 hp
  (43 each = 35 × 1.25). Note the GSC `gsc log: D;` line still prints the script's own
  (modified) number — check health, not that line.
- Spawn perk list hidden under CS rules (`weaponbar.rs` `get_perk` → "" for empty slots).
- Still open: crouch-jump hold detail (ask user: stands mid-air holding CTRL? SPACE vs
  wheel?).

## CS grenades on slot 4 (2026-10-05)
- `weapon_iw4::cs::CS_GRENADES` (index `CS_GRENADE_INDEX` = 8..10, after the knife): HE →
  MW2 twin `coltanaconda_mp` throwing `frag_grenade_mp`, flash → `beretta393_mp` /
  `flash_grenade_mp`, smoke → `pp2000_mp` / `smoke_grenade_mp`. Twin magazine = count;
  carry 1/2/1, prices 300/200/300, speed 250, slot 4 (pressing 4 again cycles them:
  `net/src/client/runtime.rs` slot selection cycles within a slot).
- Sim `cs_grenade_frame` (`combat.rs`, ReGameDLL `CHEGrenade`): attack pulls the pin
  (`PlayerState::cs_grenade` IDLE/PULLED/THROWN, netfields Exact), release throws no sooner
  than 0.5 s after the pull; throw = view pitch remapped (`-10 + p*80/90` up,
  `-10 + p*100/90` down), speed `(90-p)*6` ≤ 750, + player velocity, from eye + 16 fwd
  (`cs::grenade_throw`, `throw_cs_grenade` → `equipment::spawn_grenade_projectile_with_
  velocity`); next grenade after 0.75 s, the last one retires after 0.5 s to the best
  weapon (`item::raise_best_cs_weapon`). Knife + grenade hooks run **after** the MW2
  machine's ammo write-back (else the count reset). Buttons taken like the knife.
- Equipment overrides (`session::combat_table::apply_cs_grenade_rules`, every peer): fuse
  1.5 s, impact damage 0, HE blast 100 → 0 linear over 350 u. Verified: HE kills a dummy
  at the blast (100), thrower at ~155 u took 55; flash whites out the screen (MW2 effect);
  flash still does MW2's 1 splash damage.
- `buy hegrenade|flashbang|smokegrenade` (`cs::cs_buyable`/`cs_buy_list`); grenades stack to
  the carry limit and never take the hand (`apply_give_weapon` grenade branch).
- Viewmodels: CS:S `v_eq_fraggrenade/flashbang/smokegrenade` (mirrored like guns);
  roles `pullpin`/`throw`/draw(`deploy`); pull-pin and throw hold their last frame while
  PULLED/THROWN — **CS:S's pullpin ends with the arm below the screen** (checked by
  projecting vertices: 0 visible at its end), so the hand vanishing while you hold a pulled
  grenade is correct.
- Note: scripted screenshots land ~1 s after `screenshot` (queued); don't read timing into
  them.
- Not yet: remote players' grenade throw anim/sound is MW2's fire event only; HUD shows MW2
  names/ammo ("Low Ammo").

## Kevlar + helmet (2026-10-05)
- Rules (ReGameDLL `TakeDamage`, retail): `ARMOR_RATIO` 0.5 × weapon factor (AK 1.55, M4
  1.4, AWP 1.95, Deagle 1.5, Glock 1.05, knife 1.7, USP/other 1), `ARMOR_BONUS` 0.5 (×2
  for blasts); covered hits keep `ratio×dmg`, the vest pays `(dmg−kept)×bonus`; a vest
  that runs out stops only `armor/bonus`. Vest covers generic/chest/stomach/arms (grenade
  splash = generic), helmet the head; legs, falls never. `cs::armor_absorb`,
  `armor_penetration` (unit test `kevlar_soaks_like_cs`).
- PlayerState `cs_armor` (0-100), `cs_helmet` (netfields Exact; spawn resets them).
  Applied in GSC `finishplayerdamage` (`script_player::cs_armor_absorb`, after the
  engine-amount override, so every damage type passes through it).
- `buy vest` (650) / `buy vesthelm` (1000) → `ClientAction::BuyArmor` (wire tag 24, not
  debug-gated) → `script_player::cs_buy_armor`.
- Verified: own HE at the feet 99 → 49 through, armor 100 → 50, health 51.
- Testing note: under `IW4L_LOG_LEVEL=debug` the game runs slower than wall time, so
  scripted `wait`s cover less game time — give fuses/timers generous waits.
- Armour shows in the CS HUD (see below).

## CS:S HUD (2026-10-05, user: "bring css hud, turn off the mw2 hud, keep the minimap")
- `crates/hud/src/cs_hud.rs`: health / armour (kevlar `a`, kevlar+helmet `l`) / round
  timer / ammo panels + CS:S death notices, placed from CS:S `hudlayout.res` (640x480
  units × h/480, `r`/`c` positions; black 96-alpha rounded panels, Orange 255,176,0).
  Fonts read at runtime from `<css>/cstrike/resource/`: `cstrike.ttf` (digits + icons,
  tall 28), `cs.ttf` (grenade icons: HE `h`, flash `g`), `csd.ttf` (kill icons: ak `b`,
  m4 `w`, awp `r`, deagle `f`, usp `a`, glock `c`, knife `j`, HE `h`, headshot `D`,
  skull `C`; flash = cs.ttf `g`, dropped `DEATH_DROP` 0.44 of the size to sit level).
  Names in Windows Verdana Bold (FreeMono fallback), CT_Blue allies / T_Red axis.
  Ammo icon cut from VPK `sprites/640hud1.vtf` (lit → alpha, tinted orange): ak 762,
  m4 556, awp 338, deagle 50, usp 45, glock 9mm. Knife hides the ammo panel; grenades
  show their count + icon. Health ≤25 and timer <10s turn red. Hidden in killcam;
  dead = timer + feed only. Feed: 4 lines, 6 s (`obituary` observer, `payload.weapon`,
  headshot = `obituary_mod == MOD_HEAD_SHOT`).
- Built under the MW2 HUD root by `spawn_cs_hud` (after `ensure_hud_root`); `update_cs_hud`
  runs after the CS crosshair. `cs_hud::replaces_mw2_hud()` (= CS rules) hides the MW2
  weaponbar (ammo, weapon name, compass tickertape, perks), scorebar, splash, playercard,
  killfeed and the **blood overlay** (no regen → it would never clear). Kept: MW2 minimap
  (compass), GSC hud elems ("Eliminate other players."), overhead names, scoreboard (TAB).
- Radar decision: kept the MW2 minimap — it already is a CS:S-style radar (top-left,
  rotating map image of the real MW2 map, teammates, enemies when they fire). A
  restyle into CS:S's square radar frame is cosmetic; do it later if the user wants.
- Verified by screenshots (1920x1080): idle AK 30|90 + 7.62 icon, kill line
  "Player [AK] dummy", headshot line with the HS icon, HE "1" + icon, 20 HP red, no blood,
  dead state. Money panel (`HudAccount` r123/394) comes with rounds/money.

## Plan agreed 2026-10-05 (after user play-test of the CS HUD build)
- **No upstream merge for now**: vladtrc/iw4L has 18 new commits (375 files, 58 overlap
  ours incl. movement/net/sim; BO2 maps, late join, bot loadouts). Commit our work to the
  user's private fork first; merge upstream later in one planned session, or never.
- Crouch hold: fixed on the user's side (MW2 settings menu has a hold option; CTRL was
  bound `togglecrouch` + `toggleprone` in settings.cfg).
- **Step 1 (bug fixes, waiting for the user's "go")**:
  1. Pipe on mp_rust: small ledges slow the player (likely step-up failing on mesh lips,
     or airborne clips on facet edges). Reproduce scripted, log clip planes, fix.
  2. Ladder: weapon disappears while climbing (MW2 lowers it) → CS keeps it up and usable.
  3. Smoke hole: AWP shot (unscoped too) punches a big hole in smoke for a moment →
     MW2 sniper flash/muzzle smoke effect. Fix: MW2 flash off for CS guns + CS:S muzzle
     flash on the CS model (none yet: MDL event 5001 / barrel attachment, sprites from VPK).
     Asked: does the AK also make a hole?
  4. Hands jump while bhopping: viewmodel bob reacting to jump/land and/or eye height
     stepping per tick → CS:S-like bob, smooth eye height.
  5. Loading ~140 s: world spawn + host admission wait for the whole sound bank, which
     also loads **MW3 (iw5) sounds** (~7.4k aliases) and probes Black Ops. Fix: load only
     the map's own game's banks + `.env` switch (e.g. `IW4L_ONLY_MW2=1`) to never discover
     MW3/BO; don't block spawn on the full bank; decode CS sounds in parallel. Measure.
  6. `mv_mode csgo|surf|mmod|cs16` console presets (surf = airaccel 150, mmod = Momentum
     bhop 1000/autohop/260, cs16 = GoldSrc), saved in settings, server + prediction in sync.
- Then: M4A1/USP silencers + Glock/FAMAS burst → remaining CS weapons in batches →
  rounds/money/buy menu → C4 → CS penetration → FPS (authority thread) → commit to fork.
- **Later, after everything**: CoD4 maps (Crossfire, Citystreets) — try IW4x's converted
  IW4 zones first (read in place from the user's IW4x install; may need IW4x-format reader
  fixes); converting ourselves (OAT) or an IW3 reader only if that fails.

## Step 1 bug fixes (2026-10-05, done, awaiting user test)
- **Load time 160 s → 24 s** (mp_rust, total incl. startup; sound bank 133.6 s → 6.7 s).
  Cause was not sound: `images/IW5 weapon bundle` decoded 2193 **MW3** weapon textures
  (~125 s) through the auto-created `Modern Warfare 3.lnk` (in target/play AND in the MW2
  install folder). New `IW4L_ONLY_MW2=1` (in `.env`, `.env.example`):
  `asset_transport::only_mw2()` — `search_roots` skips shortcut roots without MW2 data,
  `link_steam_games` stops creating MW3/BO links. Read with `env_or_dotenv` because dotenvy
  stops at the unquoted `IW4L_GAMES=C:\Program Files (x86)\...` line (later keys never load!).
- **`mv_mode csgo|surf|mmod|cs16`** (`movement_iw4::rules::MovementMode`, runtime atomic; was
  the `ACTIVE` const). `rules::CS_RULES` (const true) = CS fork on (HUD/weapons/crouch);
  `rules::active()` = ruleset of the mode. Saved as `mv_mode=` in settings.cfg
  (`GameSettings::mv_mode`), applied by `console::debug_movement::sync_movement_mode`.
  Verified eye 64.06 (csgo/surf) / 53 (cs16). Listen server only — remote clients would need
  the mode replicated (not done).
- **Ladder keeps the gun**: `WeaponCmd::ladder_keeps_weapon` (sim sets it under CS rules) →
  `traversal_forces_holster` ignores LADDER. Verified on mp_rust's ladder: AK up, fires.
  New debug commands: `ladders` (ladder boxes), `smodels [filter]`, `cylinders` (pipes in
  the clip mesh, sloped first) — `console/src/feature_dispatch/hitvol.rs`.
- **Bhop hands**: MW2 camera landing dip + walk camera bob off under CS rules
  (`view_kick.rs`); CS gun bob no longer gated on ground (eased speed, `BOB_SPEED_EASE`).
- **Smoke hole / muzzle flash**: MW2 first-person flash skipped for CS guns
  (`render_frontend/.../fx/system.rs`, `cs_gun_view`); couldn't capture the hole on camera
  (screenshots stall ~1 s each) — candidates were MW2 sniper flash distortion/depth. CS:S
  flash drawn in the CS viewmodel pass: `mdl_source` parses attachments
  (`StudioModel::attachments`, `muzzle()` = "1", "2" = shell port), flash 50 ms
  (`FLASH_SECONDS`) at the muzzle: `sprites/muzzleflash4` billboard (random spin) + two crossed
  `effects/muzzleflashx` quads along shell-port→muzzle; `CsViewmodelShading::Flash`
  (`fs_viewmodel_flash`, additive, tint in the normal), quads ride an identity bone appended to
  the frame's bones. CS:S weapon scripts are encrypted `.ctx` (no per-gun flash data).
  Ask the user if the smoke hole is gone.
- **mp_rust sloped pipe** (x≈513, y 155→672, 35°, flanges): Source `StepMove` rejected the
  step-up when it landed on a flange's bevelled edge (normal 0.58) → slide into the lip
  converted the run into vz ~156 > non_jump 140 → launched, 250 → ~80. Fix (`source.rs`):
  `lip_supported` (walkable ground 2/4/6 u ahead at about the same height) accepts such steps
  and, in `categorize`, keeps a walking player grounded on the bevel; a step's kept vz is
  capped below non_jump. Test `climbing_a_slope_over_a_flange_keeps_speed_and_ground`
  (`Brushes` convex-brush test backend). In game: whole pipe climbed at 250 to the tower.
- Movement diagnostics: `IW4L_MOVE_DIAG=1` logs snags (`movement: client N lost speed ...`
  + surface profile ahead + sweeps); `IW4L_MOVE_DIAG=trace` logs every command
  (`movetrace:`). Scripted-run note: game time runs well behind wall time for the first
  seconds after load — give climbs long waits before judging "stuck".

## TODO next session (user asked, 2026-10-05)
- **Ladder strafe-climb like CS**: pressing only D (ladder at your side, looking away)
  should grab + climb. Cause: IW4 `check_ladder_move` traces along the *view* forward
  (`CheckLadderContext::forward_xy`, set from `pml.forward` in `source.rs:327` and
  `cs.rs:189`). Fix: under CS rules pass the *wish direction* (forward*fmove + right*smove,
  flattened) instead; `ladder_move` already projects strafe into climbing. Add a test in
  `source.rs` (ladder_world, strafe into the ladder climbs).
- **Destructibles off for competitive** (user 2026-10-05): cars, explosive barrels etc.
  should not explode/take damage by default; console toggle (e.g. `sv_destructibles 0|1`,
  default 0, saved) to turn them back on. Find where MW2 destructible script models
  (`maps/mp/_destructible*` GSC / destructible damage) get damage; gate under CS rules.
- **Remove the MW2 "choose class" menu** (user 2026-10-05): CS has no classes — on join/
  team pick, spawn straight in (knife + later buy menu/default pistol). Find the class menu
  flow (`console/src/class_menu.rs`, `AppScreen::ClassSelect`, GSC `menu_changeclass` /
  `_menus.gsc` response) and auto-pick a class under CS rules; keep team select for now.
- **Keybinds from the MW2 controls menu don't persist** (user 2026-10-05): user re-binds
  hold crouch/prone in the menu every launch. Check that menu bind changes write
  `settings.cfg` (`console/src/user_settings.rs` autosave) and aren't overwritten on load by
  `default_controls.cfg` / `binddefaults`; under CS rules default CTRL = `+movedown` (hold).
- **CS:S flashbang 1:1** (user 2026-10-05): replace MW2's flash whiteout with CS:S behaviour —
  blind amount/duration from distance + facing (CS rules), white screen that holds then fades,
  the frozen afterimage of the frame at the flash moment fading out, CS:S ringing sound
  (`css/` flashbang sounds), `flash_grenade` 1-dmg splash removed. Check the CS:S/ReGameDLL
  numbers (`RadiusFlash`: full blind time, fade time) before coding.
- **Smoke duration like CS:S** (user 2026-10-05: smoke look is great, keep MW2 visuals):
  match CS:S timing — detonates ~1.5 s after the throw (when it settles), full cloud ~15 s
  then fades (~20 s total; verify CS:S numbers). Adjust MW2 smoke fx lifetime / spawn under
  CS rules (`session::combat_table::apply_cs_grenade_rules`, smoke_grenade_mp effect).
  - User clarified: the key behaviour is **looking away = not flashed** (CS facing check:
    flash behind you → little/no blind; only line-of-sight flashes count).

## Overnight session 2026-10-06 (autonomous; user asleep) — progress
User's list: knife stab/backstab, ladder strafe, destructibles off, no class menu, keybinds
persist, CS:S flashbang 1:1 (look away = not flashed), smoke timing, grenade voice ("Fire in
the hole"), better grenade throw like CS, XP + challenges off, MW2 breathing/hit sounds off,
pause-menu spectate stuck bug.
- **Knife** (done, verified): stab only landed pressed against the victim — MW2 hitboxes are
  thin bone volumes, CS sweeps `head_hull` against the player *box*. `cs_knife_trace` now also
  tests lag-comped player boxes (`knife_box_hit`, ±16/±18 hull expansion, nearest of box vs
  line hits). Backstab = CS 1.6 rule: flat LOS attacker→victim · victim forward > 0.8
  (`cs::is_backstab(attacker_origin, victim_origin, victim_yaw)`). Verified stab at 45u = 65,
  backstab = 195 kill (note: `bot tp` yaw 270 left the dummy at -135).
- **Ladder strafe-climb** (done, test `strafing_into_a_ladder_at_your_side_climbs_it`):
  `ladder::cs_ladder_context` — attach trace along the move dir (fm/rm), forwardmove 127
  when moving; used by `source.rs` and `cs.rs`.
- **Destructibles off by default** (done, verified 11 hits blocked / 11 applied):
  `sim::cs_settings` (`destructibles_enabled`, `DESTRUCTIBLE_TARGETNAMES` vehicle/toy/
  destructable) gates `script::host::entity_damage::damage_entity`. Console
  `sv_destructibles 0|1` (saved, `GameSettings::destructibles`), `destructibles` lists them.
- **XP / challenges off** (done): `xblive_privatematch 1` under CS rules
  (`match_apply.rs` CS_DVARS) → `level.rankedMatch` false (no XP, rank-ups, challenges).
  New **GSC source patch table** `assets/src/cs_script_patches.rs` (exact-line replace at
  load, CS rules only, logs if an anchor is missing): `_gamescore` score popup removed.
- **Breathing muted**: `cs_settings::MUTED_LOCAL_SOUNDS` (breathing_hurt/better) skipped in
  GSC `playlocalsound`. MW2 MP has no pain grunts.
- **No class menu** (done, verified FFA + TDM): GSC `openpopupmenu changeclass*` is answered
  server-side with `class0` (`natives/player.rs`); FFA JoinMatch auto-joins
  (`step.rs`, `game_mode_kind().is_team()`); the client class overlay never opens. Team
  modes keep the team menu.
- **Spectate stuck bug** (root cause found + fixed): `deliver_answers` dropped answers for a
  spectator with no open menu → picking a team after Spectate was never heard. Now
  `PlayerSlot::answered` — once the scripts took one answer, every later answer is delivered.
  Verified TDM: join → allies (alive) → spectator → axis (alive); FFA: spectate → autoassign.
  Debug command `menuresponse <menu> <response>`; `IW4L_GAMETYPE=war` for TDM tests.
- **Keybinds** (root cause found + fixed): Left/Right Ctrl both saved as `CTRL`, and `CTRL`
  loads as both → the right side's stale `toggleprone` overwrote the hold bind every load.
  Right modifiers now save as `rctrl/rshift/ralt` (`binds::config_button_name`); menu binds
  set both sides (`set_both_sides`). Tests `modifier_tests`. User settings.cfg fixed
  (backup `settings.cfg.bak-before-ctrl-fix`).
- **CS flashbang** (done, verified facing = full white 2.0 s hold + 9.2 s fade at ~350u;
  turned away = nothing): ReGameDLL `RadiusFlash` numbers (strength 4, radius 1500, linear;
  facing ≥ 0: alpha 255, hold s/1.5, fade s×3; -0.5..0: alpha 200, hold s/3.5, fade s×1.75)
  + CS:S rule looking away (< -0.5) = no flash; stacking like `RadiusFlash`
  (`cs::flash_for/stack_flash/flash_screen`, test `flashbangs_follow_cs_facing_and_distance`).
  Server `damage::apply_cs_flash` (replaces MW2's GSC `flashbang` notify → no MW2 shellshock),
  PlayerState `cs_flash_start_ms/hold_ms/fade_ms/alpha` (netfields Exact); HUD `flash.rs`
  draws white (hold then linear) + the frozen frame (sqrt fade, outlasts the white). Flash
  splash damage 0 (`apply_cs_grenade_rules`). No CS:S ear-ringing yet.
- **Smoke ~18 s like CS:S** (verified by timeline screenshots): MW2's cloud is gone ~12 s after
  it pops, so the server re-fires the explosion event once at +6.5 s
  (`equipment::CsSmoke`, `refire_cs_smokes` after entity thinks). Re-fired event rides the
  thrower's entity number (clients drop events whose entity is gone).
- **"Fire in the hole!"** (done, verified start in dump): CS:S `radio.*` sound entries now
  loaded (`append_css_weapon_sounds`), `css/radio.fireinthehole` played locally on the
  PULLED→THROWN transition (`cs_viewmodel.rs`). Teammates don't hear it yet (needs a
  server-sent sound).
- **CS grenade physics**: new trajectory `entity_iw4::TR_GRAVITY_HALF` (0x20, half gravity,
  `is_gravity()` helper; killcam uses it) for thrown CS grenades (`spawn_grenade_projectile_
  with_velocity`); bounce data: parallel 0.8, perpendicular 0.3 (HE) / 0.2 (flash, smoke).
  Verified HE thrown level reaches ~450u (bot hit).
- Fall damage: MW2 MP plays no fall/pain sound; the "breathing after a hit" was the GSC
  breathing (muted). Bots: 3 bots spawn and live (no weapons → no kills, pre-existing).
