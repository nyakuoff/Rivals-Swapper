# Rivals Swapper

A Marvel Rivals skin changer tool that lets you swap character skins by generating mod files (`.pak` / `.utoc` / `.ucas`) using **repak-rivals**.

> **Client-side cosmetic only** | does not modify game logic or provide any competitive advantage.

---

## Features

- **One-click skin swap** — pick a character, pick a skin, press Swap
- **Auto-packs** with repak-rivals (`.pak` → IOStore)
- **Auto-deploys** to the game's `~mods` folder
- **Online skin database** — fetches latest skins from the natimerry API

## Prerequisites

| Tool | Source | Notes |
|------|--------|-------|
| **Python 3.11+** | [python.org](https://python.org) | |
| **repak.exe** | Already included | `tools/Repak/CLI/repak.exe` |
| **repak-gui.exe** | Already included | `tools/Repak/GUI/repak-gui.exe` |

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

1. Open the app and click **⚙ Settings**
2. Set **Game Paks Folder** — the directory containing the game's `.pak` files  
   Example: `C:\Program Files (x86)\Steam\steamapps\common\MarvelRivals\MarvelGame\Marvel\Content\Paks`
3. *(Optional)* Set **Extracted Content** — if you've extracted game files with FModel, point to the output root so the tool can copy real mesh data instead of stubs

## How It Works

1. **Skin Database** pulls character/skin IDs from [rivals.natimerry.com/skins](https://rivals.natimerry.com/skins)
2. **Swap Engine** mirrors the original folder tree to replace the default skin's mesh files with the chosen skin's meshes
3. **repak-gui.exe --pack** converts the folder to IOStore format (`.pak` + `.utoc` + `.ucas`) in one automated step — no manual drag-and-drop needed
4. The final files are copied to the game's `~mods` folder

> [!NOTE]
> **AI Disclaimer**: Parts of this project were assisted or written by AI. If that's something you're not comfortable with, no hard feelings, I understand and I don't force anyone to use it. The code may have flaws. If you spot something that could be better, contributions are very welcome. I'm still learning and would appreciate the help.