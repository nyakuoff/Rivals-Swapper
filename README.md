# Rivals Swapper

A Marvel Rivals skin changer tool that lets you swap character skins.

> **Client-side cosmetic only** | does not modify game logic or provide any competitive advantage.

---

## Features

- **One-click skin swap** — pick a character, pick a skin, press Swap
- **Auto-packs** with repak-rivals
- **Auto-deploys** to the game's `~mods` folder
- **Local skin database** — built from game files, no API key required

## Prerequisites

| Tool | Source | Notes |
|------|--------|-------|
| **Python 3.11+** | [python.org](https://python.org) | |
| **repak.exe** | Already included | `tools/Repak/CLI` |
| **repak-gui.exe** | Already included | `tools/Repak/GUI` |
| **umodel** | Already included | `tools/umodel` |
| **FModel** | [fmodel.app](https://fmodel.app) |
| **Project Galacta** | Auto-downloaded | Mod + Companion App fetched from [GitHub Releases](https://github.com/0xSaturno/ProjectGalacta/releases) into `tools/galacta`; the mod is installed into `~mods` automatically |

## How swaps work (Project Galacta)

Swaps are built as [Project Galacta](https://0xsaturno.github.io/ProjectGalacta/) Skin Swapper mods: the source skin's mesh is copied to `/Game/Marvel/ProjectGalacta/Meshes/{HeroID}/{DefaultSkinID}/{Mod}/` alongside a `GAL_ModSkinConfig` asset, without overriding any vanilla files.

Rivals Swapper downloads and updates Project Galacta on each swap. The **Galacta** button opens the Companion App (through Wine on Linux; it needs the .NET 8 Desktop Runtime), which rescans your mods on launch.

After swapping:
1. Click **Launch**. If your mods changed since the last manifest write, the Companion opens and scans them; click **Write** in its Manifest tab and the game starts once `skins.txt` is written. (Skins only work after they've been written, not just scanned.)
2. In-game, equip the hero's **default** skin, press **P**, and pick the swapped skin.

A hero can have several swapped skins at once; they all show up in the Skin Swapper's list.

Only skins with their own mesh can be swapped; Galacta doesn't support retexture-only skins or weapon swaps.

## Quick Start

```bash
# 1. Clone / download the project
cd Rivals-Swapper

# 2. Create a virtual environment (recommended)
python -m venv venv
venv\Scripts\activate        # Windows

# 3. Install dependencies
pip install -r requirements.txt

# 4. Run the app
python main.py
```

## First-Time Setup

1. Open the app and open the **Settings**
2. Set **Game Paks Folder** — the directory containing the game's `.pak` files  
   Example: `C:\Program Files (x86)\Steam\steamapps\common\MarvelRivals\MarvelGame\Marvel\Content\Paks`

> **Note:** On first launch the app will automatically extract skin images from the game paks using umodel. This takes a minute or two and only runs once (or after a game update).

## Docs

- [Updating the database](docs/updating-the-database.md) — You can manually rebuild the database if i haven't updated iit yet.

> [!NOTE]
> **AI Disclaimer**: Parts of this project were assisted or written by AI. If that's something you're not comfortable with, no hard feelings, I understand and I don't force anyone to use it. The code may have flaws. If you spot something that could be better, contributions are very welcome. I'm still learning and would appreciate the help.