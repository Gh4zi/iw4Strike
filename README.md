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

---

## Features

**Movement**
- 100 Hz server tick (MW2 runs at 20 Hz)
- CS:GO-style movement by default: bunny hopping, air strafing, stamina, duck-jumping
- Other presets: surf, Momentum Mod bhop and CS 1.6 movement (`mv_mode`)
- CS ladders: strafe onto a ladder to climb it, and your gun stays up
- No sprint, no prone

**Weapons**
- AK-47, M4A1, AWP, Desert Eagle, USP and Glock-18 with Counter-Strike: Source models and
  sounds, and CS 1.6 damage, spread and recoil (one-tap AK headshots)
- AWP scope: instant two-level zoom with the CS scope overlay
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
Counter-Strike 1.6 install. The CS:S HUD needs CS:S.

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
IW4L_CSS="D:\SteamLibrary\steamapps\common\Counter-Strike Source\cstrike"
```

Point `IW4L_CSS` at the **`cstrike` folder inside** `Counter-Strike Source`, not at
`Counter-Strike Source` itself. iw4Strike only reads from this folder and never changes or
copies its files.

### 4. Build and play

```bash
cargo run --profile play -p launcher -- map mp_boneyard --cmds "wait world; spawn 0; force_match_start; bot add 3"
```

This builds the game and starts a match on Boneyard with three bots. The first build takes
a while. With GNU Make installed, `make map mp_boneyard CMDS='...'` does the same.

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

| Command | What it does |
|---|---|
| `buy <weapon>` | `ak47` `m4a1` `awp` `deagle` `usp` `glock` `hegrenade` `flashbang` `smokegrenade` `vest` `vesthelm` |
| `mv_mode <mode>` | Movement style: `csgo` (default), `surf`, `mmod` (Momentum bhop), `cs16` |
| `viewmodel_fov <54-90>` | How far away your gun is drawn (default 68) |
| `cl_wpn_sway 0\|1` | Gun bob and sway on or off |
| `sv_destructibles 0\|1` | Let cars and barrels explode again (off by default) |
| `binddefaults` | Reset all keys to the iw4Strike defaults |

There's no money system yet, so `buy` is free for now.

---

## Roadmap

- [x] CS movement, 100 Hz tick
- [x] AK-47, M4A1, AWP, Deagle, USP, Glock-18
- [x] Knife, HE, flashbang, smoke
- [x] Kevlar and helmet, drop and pick up
- [x] Counter-Strike: Source HUD
- [ ] M4A1 and USP silencers, Glock and FAMAS burst fire
- [ ] The rest of the CS weapons
- [ ] Rounds, money and buy menu
- [ ] C4 bomb plant and defuse
- [ ] Shooting through walls (CS penetration)
- [ ] Better FPS
- [ ] CoD4 maps (Crossfire, Citystreets)

**Known issues**
- Other players see MW2 weapon models in your hands, not the CS ones.
- The movement mode (`mv_mode`) isn't sent to other players yet, so use the same mode on
  every client.

Detailed progress notes are in [`CLAUDE.md`](CLAUDE.md).

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

- Built on [IW4L](https://github.com/vladtrc/iw4L) by vladtrc and its contributors.
- IW4L is licensed under Apache 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
- Call of Duty: Modern Warfare 2 belongs to Activision. Counter-Strike and
  Counter-Strike: Source belong to Valve. iw4Strike isn't affiliated with either, and you
  need your own copies of both games.
