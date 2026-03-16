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
| **repak.exe** | Already included | `tools/Repak/CLI/repak.exe` |
| **repak-gui.exe** | Already included | `tools/Repak/GUI/repak-gui.exe` |
| **umodel** | Already included | `tools/umodel/umodel_materials_ue5.exe` — exports skin images from game paks |
| **FModel** | [fmodel.app](https://fmodel.app) | Only needed when rebuilding the database after a game update |

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

- [Updating the database](docs/updating-the-database.md) — how to rebuild `data/game_database.json` after a Marvel Rivals update using FModel

> [!NOTE]
> **AI Disclaimer**: Parts of this project were assisted or written by AI. If that's something you're not comfortable with, no hard feelings, I understand and I don't force anyone to use it. The code may have flaws. If you spot something that could be better, contributions are very welcome. I'm still learning and would appreciate the help.