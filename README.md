# Rivals Swapper

A Marvel Rivals skin changer tool that lets you swap character skins.

> **Client-side cosmetic only** | does not modify game logic or provide any competitive advantage.

---

## Features

- **One-click skin swap**: pick a character, pick a skin, press Swap
- **Nothing to click in-game**: the swap replaces the hero's default skin, so equipping the default skin is all it takes. No extra in-game mod or companion app
- **Automatic skin database**: read straight from the game's localization and rebuilt by itself after every game update, so there's nothing to export or download
- **Swaps recolors too**: recolor skins are rebuilt from the skin they recolor, and multi-form heroes are covered form by form
- **Native and light**: a single Rust binary for Windows and Linux, around 100 MB of RAM while idle

## Prerequisites

| Tool | Source | Notes |
|------|--------|-------|
| **umodel** | Already included | `tools/umodel`, exports skin images (runs through **Wine** on Linux) |
| **UAssetTool** | Already included | `tools/uassettool`, packs the mods |
| **UTOC signature bypass** | Community tool, install it yourself | Season 10 only loads mods with it installed (e.g. `dsound.dll` + `plugins/MarvelRivalsUTOCSignatureBypass.asi` in `Marvel/Binaries/Win64`; on Proton add `WINEDLLOVERRIDES="dsound=n,b" %command%` to the launch options) |

## How swaps work

A swap is a regular `~mods` mod that replaces the hero's **default** skin mesh: the source skin's mesh is moved onto the default mesh's own path and object name (`/Game/Marvel/Characters/{Hero}/{Hero}001/Meshes/...`), keeping its own materials. The game then draws it natively whenever the default skin is equipped; nothing runs in-game. Mods under `/Game/Marvel/Characters/` load as-is with the signature bypass, so they aren't encrypted.

- **Lobby too.** The lobby (menu) model is replaced the same way, so the swapped skin also shows in the hero gallery and lobby.
- **Sounds too.** Skins with their own ability sound effects (mostly legendary ones) bring them along: their sound tables replace the default skin's.
- **Effects too.** Skins with their own ability VFX can bring them along: their gameplay cues replace the default skin's, the default animations play the skin's effects and sounds, and a shared skin-table mod (`RivalsSwapper_SkinTable`) points the default skin at the skin's blueprint for its body effects and effect colors.
- **One swap per hero.** Swapping another skin for the same hero replaces the previous swap.
- **Recolors** (no mesh of their own) are swapped as the skin they recolor, with the mesh's material slots pointed at the recolor's materials.
- **Multi-form heroes** (Hulk, Cloak & Dagger, ...) get every form replaced.
- **Weapons** keep their default look.

Swaps made with older versions that used Project Galacta keep working as long as the Galacta mod stays in `~mods`; swapping that hero again replaces them.

## Quick Start

Download the latest release for your OS, extract it, and run `RivalsSwapper`.

To build from source (Rust 1.88+):

```bash
cargo run --release
```

## First-Time Setup

The app finds Marvel Rivals in your Steam libraries on its own. If it doesn't, open **Settings** and set the **Game Paks Folder**, for example:
`C:\Program Files (x86)\Steam\steamapps\common\MarvelRivals\MarvelGame\Marvel\Content\Paks`

> **Note:** On first launch, and after each game update, the app rebuilds its skin database (a couple of seconds) and exports skin images with umodel (under a minute). Both are cached in `~/.cache/RivalsSwapper` (`%LOCALAPPDATA%\RivalsSwapper` on Windows).

To check the database without the GUI:

```bash
rivals-swapper --dump-db skins.json
```

> [!NOTE]
> **AI Disclaimer**: Parts of this project were assisted or written by AI. If that's something you're not comfortable with, no hard feelings, I understand and I don't force anyone to use it. The code may have flaws. If you spot something that could be better, contributions are very welcome. I'm still learning and would appreciate the help.