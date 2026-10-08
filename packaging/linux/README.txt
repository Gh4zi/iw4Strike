iw4Strike - Counter-Strike gameplay on Modern Warfare 2 maps (Linux)
https://github.com/Gh4zi/iw4Strike

YOU NEED
  - Call of Duty: Modern Warfare 2 (2009) on Steam. It is a Windows game:
    in Steam, open its Properties > Compatibility, tick "Force the use of
    a specific Steam Play compatibility tool" and install it. iw4Strike
    only reads its files; MW2 itself never runs.
  - Counter-Strike: Source on Steam (weapon models, sounds, HUD). The
    native Linux version is fine.
  - A Vulkan graphics driver (Mesa or NVIDIA), and a 64-bit distribution
    as new as Ubuntu 22.04 or newer.
  - zenity or kdialog, for the Browse... buttons (most desktops have one).

HOW TO PLAY
  1. Extract this folder somewhere you can write to, for example
     ~/Games:  tar xzf iw4Strike-*-linux.tar.gz
  2. Run ./iw4strike from that folder (or double-click it).
  3. The first time, a "game folders" window shows what was found. MW2 and
     Counter-Strike: Source are found in your Steam libraries
     (~/.local/share/Steam, Flatpak and Snap Steam too, and every library
     folder Steam knows about).
     - Not found: press Browse... and select the game's folder.
     - No CS:Source at all: select your Half-Life folder (Counter-Strike 1.6)
       instead. It is used when CS:Source is missing or its Use box is off.
     Then press Play. Open the window again from Options > Game Folders,
     or with ./iw4strike paths.
     MW2 can also be linked by hand, next to iw4strike:
       ln -s "/path/to/Call of Duty Modern Warfare 2" "Modern Warfare 2"

IN GAME
  - Press the key under Esc (`) to open the console.
  - Commands: buy ak47, buy vesthelm, bot add 3, mv_mode csgo, map mp_rust
  - Full list of commands and controls:
    https://github.com/Gh4zi/iw4Strike#console-commands

Settings, demos and logs are saved in the iw4l-artifacts folder that
appears next to iw4strike.

Built on IW4L (https://github.com/vladtrc/iw4L), Apache 2.0 - see LICENSE
and NOTICE. MW2 belongs to Activision; Counter-Strike belongs to Valve.
No game files are included.
