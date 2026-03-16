# Updating the Database

`data/game_database.json` is built from Marvel Rivals' localization file (`Game.json`) using the script at `scripts/build_database.py`. Run this after each game update to pick up new heroes and skins.

---

## Prerequisites

- **FModel** — free UE asset browser: https://fmodel.app
- Python 3.11+ with the project's venv active (`pip install -r requirements.txt`)

---

## Step 1 — Export Game.json with FModel

1. Open FModel and load **Marvel Rivals**.
2. In the Archives tab, make sure the game AES key is entered.
3. Navigate to:
   ```
   Marvel/Content/Localization/Game/en/
   ```
4. Right-click **Game.uasset** → **Export raw data** (or **Save Properties**).
5. FModel writes `Game.json` to its output folder, typically one of:
   ```
   %USERPROFILE%\Desktop\MarvelModding\FModel\Output\Exports\Marvel\Content\Localization\Game\en\Game.json
   %LOCALAPPDATA%\FModel\Output\Exports\Marvel\Content\Localization\Game\en\Game.json
   ```

---

## Step 2 — Run the build script

If `Game.json` is in one of the default locations above, just run:

```powershell
python scripts/build_database.py
```

Otherwise pass the path explicitly:

```powershell
python scripts/build_database.py "C:\path\to\Game.json"
```

The script prints a summary and overwrites `data/game_database.json`:

```
Reading Game.json …
  82,000 total localization keys
  430 skin name entries
  49 char name entries
  49 characters in database

Wrote data/game_database.json
  49 characters, 430 total skins
```

---

## Step 3 — Verify the output

Open `data/game_database.json` and spot-check:

- New heroes appear with the correct display name.
- New skins are listed under the right hero.
- No obviously wrong names (e.g. raw key strings like `CHAR_NAME_1055`).

---

## Fixing wrong or missing names

### Wrong hero name

Add an entry to `CHAR_NAME_OVERRIDES` near the top of `build_database.py`:

```python
CHAR_NAME_OVERRIDES: dict[str, str] = {
    "1055": "Daredevil",   # example: localization stored an unexpected string
}
```

The `char_id` is the 4-digit ID visible in the JSON (e.g. `"char_id": "1055"`).

### Missing hero

If a brand-new hero's name isn't picked up yet, add a temporary override:

```python
CHAR_NAME_OVERRIDES: dict[str, str] = {
    "1065": "New Hero Name",
}
```

Remove the override once the localization catches up.

### Missing skin

For skins not yet in the localization, add to `HARDCODED_SKINS`:

```python
HARDCODED_SKINS: list[tuple[str, str, str]] = [
    ("1055", "1055800", "Born Again Season 2"),
]
```

Tuple format: `(char_id, skin_id, display_name)`.

### Blacklisted skin names

Skins whose names contain any string in `SKIN_NAME_BLACKLIST` are excluded from the output. Add substrings there if a cosmetic variant should be hidden:

```python
SKIN_NAME_BLACKLIST = ["Cosmic Invasion", "some other variant"]
```

---

## After updating

Re-run the app. On first launch it will re-export textures from the game paks and rebuild the image cache automatically.

If skin images look stale, delete `data/images/` to force a full re-cache.
