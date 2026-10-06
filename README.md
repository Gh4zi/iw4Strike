# iw4Strike

**Counter-Strike gameplay on Call of Duty: Modern Warfare 2 maps.**

iw4Strike is a fork of [IW4L](https://github.com/vladtrc/iw4L), an open-source Modern
Warfare 2 (2009) engine written in Rust. It keeps MW2's maps and replaces the gameplay
with Counter-Strike: CS movement, CS weapons and damage, the knife, grenades, armor and the
Counter-Strike: Source HUD.

Weapon models, sounds and HUD fonts are read at runtime from **your own Counter-Strike:
Source install**. Gameplay numbers (damage, spread, recoil, speeds) follow CS 1.6. No game
files are included in this repository.

> **Work in progress.** Rounds, money, the buy menu and the bomb are not in yet. See
> [Roadmap](#roadmap).

## Download

**Just want to play?** Get the Windows build from
[**Releases**](https://github.com/Gh4zi/iw4Strike/releases). Extract the zip, run `iw4l.exe`,
and see `README.txt` inside. You still need MW2 and Counter-Strike: Source on Steam (see
[What you need](#what-you-need)). To build it yourself, see [Setup](#setup).

---

## Features

**Movement**
- 100 Hz server tick (MW2 runs at 20 Hz)
- CS:GO-style movement by default: bunny hopping, air strafing, stamina, duck-jumping
- Other presets: surf, Momentum Mod bhop and CS 1.6 movement (`mv_mode`)
- CS ladders: strafe onto a ladder to climb it, and your gun stays up
- No sprint, no prone

**Weapons**
- 24 guns with Counter-Strike: Source models and sounds, and CS 1.6 damage, spread and recoil
  (one-tap AK headshots): pistols, shotguns, SMGs, rifles, snipers and the M249
- M4A1 and USP silencers (right click), Glock-18 and FAMAS burst fire (right click)
- Scopes: instant zoom with the CS scope overlay on the AWP, Scout, G3/SG-1 and SG 550; the
  AUG and SG 552 zoom to 55 and keep the gun in view
- Shotguns load one shell at a time
- Knife: slash 15, stab 65, backstab 195
- HE grenade, flashbang and smoke grenade with CS throw physics
  - Flashbangs follow CS rules: if you look away, you don't get flashed
  - Smoke lasts about 18 seconds, like CS:S
- Kevlar and helmet
- Drop your gun with **G**, walk over a gun to pick it up, or press **E** to swap

**HUD and game**
- Counter-Strike: Source HUD: health, armor, ammo, round timer, kill feed and crosshair
- MW2 minimap kept as the radar
- MW2 perks, killstreaks, XP, challenges, class menu and exploding cars and barrels are
  turned off

---

## What you need

| | |
|---|---|
| **Call of Duty: Modern Warfare 2** (2009, Steam) | Maps and engine data |
| **Counter-Strike: Source** (Steam) | Weapon models, sounds and HUD fonts |
| **Windows** | Steam games are found automatically on Windows |
| **Rust** ([rustup](https://rustup.rs)) | To build the game |
| **Visual Studio Build Tools** | Pick the *Desktop development with C++* workload |

If Counter-Strike: Source isn't installed, the models and sounds fall back to a
Counter-Strike 1.6 install, which you select yourself (see below). The CS:S HUD needs CS:S.

---

## Setup

### 1. Get the code

```bash
git clone https://github.com/Gh4zi/iw4Strike.git
cd iw4Strike
```

### 2. Point it at your games

Copy `.env.example` to `.env`:

```bat
copy .env.example .env
```

Open `.env` and set these:

```ini
# The folder that contains your Modern Warfare 2 folder
IW4L_GAMES="C:\Program Files (x86)\Steam\steamapps\common"

# Load only MW2 (skips MW3 / Black Ops; much faster map loading)
IW4L_ONLY_MW2=1
```

**Put quotes around paths.** A path with spaces or brackets and no quotes stops `.env` from
loading the lines after it.

### 3. Counter-Strike: Source folder

The game looks for Counter-Strike: Source in all your Steam libraries:

```
<Steam library>\steamapps\common\Counter-Strike Source\cstrike
```

That `cstrike` folder must contain `cstrike_pak_dir.vpk`.

If it isn't found (for example, it's installed outside Steam), add its `cstrike` folder to
`.env`:

```ini
IW4L_CSS="D:/SteamLibrary/steamapps/common/Counter-Strike Source/cstrike"
```

Point `IW4L_CSS` at the **`cstrike` folder inside** `Counter-Strike Source`, not at
`Counter-Strike Source` itself. iw4Strike only reads from this folder and never changes or
copies its files.

**No Counter-Strike: Source?** Select your Counter-Strike 1.6 `cstrike` folder (the one with a
`models` folder inside, usually `<Steam library>\steamapps\common\Half-Life\cstrike`) in
`.env`:

```ini
IW4L_CSTRIKE="C:/Program Files (x86)/Steam/steamapps/common/Half-Life/cstrike"
```

It is never searched for, and it is ignored whenever Counter-Strike: Source is found.

### 4. Build and play

```bash
cargo run --profile play -p launcher -- map mp_boneyard --cmds "wait world; spawn 0; force_match_start; bot add 3"
```

This builds the game and starts a match on Boneyard with three bots. The first build takes
a while. With GNU Make installed, `make map mp_boneyard CMDS='...'` does the same.

### Automatic Windows builds

GitHub builds `iw4l.exe` for you with the
[Windows build](.github/workflows/windows-build.yml) workflow:

- **Publish a release** (Releases → *Draft a new release* → new tag such as `v0.2.0` →
  *Publish release*). About 30–60 minutes later the zip is attached to that release.
- **Push code to `main`.** The zip is built and kept for 14 days under
  [Actions](https://github.com/Gh4zi/iw4Strike/actions/workflows/windows-build.yml) → the
  run → *Artifacts*.
- **Build by hand:** Actions → *Windows build* → *Run workflow*. Fill in a release tag to
  attach the zip to that release.

---

## Controls

| Key | Action |
|---|---|
| **W A S D** | Move |
| **Space** / **Mouse wheel down** | Jump |
| **Ctrl** | Crouch |
| **Shift** | Walk |
| **Mouse 1** | Fire / knife slash |
| **Mouse 2** | AWP zoom / knife stab |
| **R** | Reload |
| **E** | Use / swap for a gun on the ground |
| **G** | Drop gun |
| **1 / 2 / 3 / 4** | Primary / pistol / knife / grenades (press 4 again to cycle grenades) |
| **Tab** | Scoreboard |

Your keys still the old MW2 ones? Run `binddefaults` in the console.

---

## Console commands

Press **`** (the key under **Esc**) to open the console. Type a command and press
**Enter**.

> Commands marked 🔧 need cheats. Cheats are **on by default** in your own matches. A lobby
> host can turn them off in the game setup, or you can start the game with `--no-cheats`.

### 🛒 Buy weapons and armor

Use `buy <name>`. There's no money yet, so buying is free. The price is what it will cost
once money is added.

| Type | Name | Weapon | Price |
|---|---|---|---|
| Rifle | `ak47` | AK-47 | $2500 |
| Rifle | `m4a1` | M4A1 | $3100 |
| Sniper | `awp` | AWP | $4750 |
| Pistol | `deagle` | Desert Eagle | $650 |
| Pistol | `usp` | USP | $500 |
| Pistol | `glock` | Glock-18 (right click: burst) | $400 |
| Pistol | `p228` | P228 | $600 |
| Pistol | `fiveseven` | Five-seveN | $750 |
| Pistol | `elite` | Dual Elites | $800 |
| Shotgun | `m3` | M3 | $1700 |
| Shotgun | `xm1014` | XM1014 | $3000 |
| SMG | `tmp` | TMP | $1250 |
| SMG | `mac10` | MAC-10 | $1400 |
| SMG | `mp5` | MP5 | $1500 |
| SMG | `ump45` | UMP45 | $1700 |
| SMG | `p90` | P90 | $2350 |
| Rifle | `galil` | Galil | $2000 |
| Rifle | `famas` | FAMAS (right click: burst) | $2250 |
| Rifle | `sg552` | SG 552 (right click: scope) | $3500 |
| Rifle | `aug` | AUG (right click: scope) | $3500 |
| Sniper | `scout` | Scout | $2750 |
| Sniper | `sg550` | SG 550 | $4200 |
| Sniper | `g3sg1` | G3/SG-1 | $5000 |
| Machine gun | `m249` | M249 | $5750 |
| Grenade | `hegrenade` | HE grenade (carry 1) | $300 |
| Grenade | `flashbang` | Flashbang (carry 2) | $200 |
| Grenade | `smokegrenade` | Smoke grenade (carry 1) | $300 |
| Armor | `vest` | Kevlar | $650 |
| Armor | `vesthelm` | Kevlar + helmet | $1000 |

```
buy ak47
buy deagle
buy vesthelm
buy flashbang
```

All `buy` commands are 🔧 for now.

### 🏃 Movement

| Command | What it does |
|---|---|
| `mv_mode csgo` | CS:GO competitive movement, the default (stamina, no auto-hop) |
| `mv_mode cs16` | Counter-Strike 1.6 movement (smaller player, lower jump) |
| `mv_mode surf` | Surf settings (very high air control, auto-hop) |
| `mv_mode mmod` | Momentum Mod bhop (auto-hop, no stamina, 260 speed) |
| `mv_stamina <0-1>` | How much jumping slows you down: `1` normal, `0.5` half, `0` off |

Your movement mode is saved, so you only set it once.

### 🔫 Gun and crosshair

| Command | What it does |
|---|---|
| `viewmodel_fov <54-90>` | Gun position: bigger moves the gun further away (default `68`) |
| `cl_wpn_sway 0` / `1` | Turn gun bob and sway off or on |
| `cl_dynamiccrosshair 0` / `1` | Static crosshair, or one that opens when you move and shoot |
| `thirdperson 1` / `0` | Third-person view on or off |

### 🗺️ Maps and bots

| Command | What it does |
|---|---|
| `map <name>` | Load a map, for example `map mp_rust` |
| `map_restart` | Restart the match on the same map |
| `bot add <count>` | Add bots that move and fight, for example `bot add 5` |
| `bot dummy <count>` | Add bots that stand still (for aim practice) |
| `force_match_start` | 🔧 Skip the pre-match countdown |
| `disconnect` | Leave the match and go back to the main menu |
| `quit` | Close the game |

<details>
<summary><b>MW2 map names</b></summary>

| Map | Name | Map | Name |
|---|---|---|---|
| Afghan | `mp_afghan` | Rundown | `mp_rundown` |
| Derail | `mp_derail` | Rust | `mp_rust` |
| Estate | `mp_estate` | Scrapyard | `mp_boneyard` |
| Favela | `mp_favela` | Skidrow | `mp_nightshift` |
| Highrise | `mp_highrise` | Sub Base | `mp_subbase` |
| Invasion | `mp_invasion` | Terminal | `mp_terminal` |
| Karachi | `mp_checkpoint` | Underpass | `mp_underpass` |
| Quarry | `mp_quarry` | Wasteland | `mp_brecourt` |

</details>

### 🎯 Practice

| Command | What it does |
|---|---|
| `god` | 🔧 Toggle invincibility |
| `kill` | 🔧 Kill yourself (respawn) |
| `showpos` | Show your position on screen |
| `destructibles` | List the map's cars, barrels and breakable walls |
| `sv_destructibles 0` / `1` | Cars and barrels can't explode (default) / can explode |

A quick aim-practice setup:

```
bot dummy 3
buy ak47
buy vesthelm
god
```

### ⌨️ Keys

| Command | What it does |
|---|---|
| `bind <key> <command>` | Bind a key, for example `bind Q slot3` (knife on Q) |
| `unbind <key>` | Remove a key's bind |
| `binddefaults` | Reset every key to the iw4Strike defaults |

### 🎬 Demos and clips

| Command | What it does |
|---|---|
| `record <name>` | Start recording a demo |
| `demo <name>` | Play a recorded demo |
| `clip` | Save the last 45 seconds as a clip |

Demos and clips are saved in the `iw4l-artifacts` folder.

---

## Roadmap

- [x] CS movement, 100 Hz tick
- [x] AK-47, M4A1, AWP, Deagle, USP, Glock-18
- [x] Knife, HE, flashbang, smoke
- [x] Kevlar and helmet, drop and pick up
- [x] Counter-Strike: Source HUD
- [x] M4A1 and USP silencers, Glock and FAMAS burst fire
- [x] The rest of the CS weapons (pistols, shotguns, SMGs, rifles, snipers, M249)
- [ ] Other players holding the CS weapon models in third person
- [ ] Movement mode (`mv_mode`) as a server-side setting that clients can't change, so every
  player moves the same way
- [ ] Rounds, money and buy menu
- [ ] C4 bomb plant and defuse
- [ ] Shooting through walls (CS penetration)
- [ ] Better FPS
- [ ] CoD4 maps (Crossfire, Citystreets)

**Known issues**
- Smoke grenades go off twice: the explosion is fired a second time about 6.5 seconds later so
  the cloud lasts about 18 seconds, like CS:S. This will be reworked to match the CS:S smoke
  timer properly.
- Flashbangs and smoke grenades don't bounce like CS grenades yet. Their bouncing will be
  reworked.
- Terminal: broken glass can turn red when you look at it from certain angles. This may be
  one of the reasons for low FPS.
- Favela: rendering issues and low FPS (cause not found yet).
- Fall damage is inconsistent: sometimes a fall kills you, sometimes it doesn't register any hit.
- There is no fall damage sound yet (to come from CS 1.6 or CS:S).
- The flashbang explosion has no sound yet.
- The map ambient sound is too loud on every map. For example, the airplane sound on Terminal is
  too high and Highrise is very loud and windy. It may be related to the 100 Hz tick rate (not
  confirmed).
- Walking with **Shift** isn't silent yet. In CS, walking makes no footstep sounds.
- Walking over a weapon doesn't pick it up automatically; you have to press **F**.
- Dropping and throwing a weapon (**G**) will be reworked.

---

## How it works

iw4Strike runs on IW4L's engine, which reads MW2's own game files directly. The CS parts
live in these crates:

| Area | Where |
|---|---|
| CS movement | `crates/movement_iw4/src/source.rs`, `cs.rs`, `rules.rs` |
| CS weapons and knife | `crates/weapon_iw4/src/cs.rs`, `crates/sim/src/combat.rs` |
| CS:S model loading (VPK, MDL, VTF) | `crates/mdl_source` |
| CS 1.6 model loading (fallback) | `crates/mdl_goldsrc` |
| CS sounds | `crates/asset_audio/src/cs_sounds.rs` |
| CS:S HUD and crosshair | `crates/hud/src/cs_hud.rs`, `cs_crosshair.rs` |
| Gun drawing and AWP scope | `crates/render_gpu/src/drawsurf/cs_viewmodel.rs`, `cs_scope.rs` |

IW4L's engine docs are in [`docs/`](docs/INDEX.md), and its README is
[upstream](https://github.com/vladtrc/iw4L#readme).

---

## Credits and license

### Engine

- **[IW4L](https://github.com/vladtrc/iw4L)** by vladtrc and contributors: the MW2 engine
  iw4Strike is built on. IW4L's own credits (OpenAssetTools, IW4x, KisakCOD and others) are
  in its [README](https://github.com/vladtrc/iw4L#acknowledgements-and-license) and in
  [NOTICE](NOTICE).

### Gameplay references

iw4Strike's Counter-Strike gameplay is written from scratch in Rust. These projects were
read to get the behaviour and numbers right. None of their source code is included here.

| Project | What it was used for |
|---|---|
| **[Momentum Mod](https://github.com/momentum-mod/game)** | Source / CS:GO movement (`mom_gamemovement.cpp`): move order, jumping, friction, air acceleration, step-up, ramp and slope fixes, ducking. The bhop, surf and KZ stamina settings behind `mv_mode`. The dynamic crosshair (`hud_crosshair.cpp`). |
| **[ReGameDLL_CS](https://github.com/s1lentq/ReGameDLL_CS)** | Counter-Strike 1.6 rules: weapon stats (`wpn_*.cpp`, `weapons.h`), damage, range and hitgroups, spread and recoil, kevlar and helmet, knife, grenades, flashbang blinding (`RadiusFlash`), AWP zoom, dropping and picking up weapons, CS 1.6 movement numbers and fall damage. |
| **[hlsdk-portable](https://github.com/FWGS/hlsdk-portable)** | GoldSrc player movement (`pm_shared`) and the Half-Life model format (`studio.h`), used by the CS 1.6 movement mode and the CS 1.6 model fallback. |
| **[Source SDK 2013](https://github.com/ValveSoftware/source-sdk-2013)** | Source engine movement (`gamemovement.cpp`) and the public headers for Source model files, used to load Counter-Strike: Source models. |

### Games

- Call of Duty: Modern Warfare 2 belongs to Activision. Counter-Strike and
  Counter-Strike: Source belong to Valve. iw4Strike isn't affiliated with any of them.
  You need your own copies of the games, and their files are read from your install,
  never copied.

### License

IW4L is licensed under Apache 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
