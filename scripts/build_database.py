#!/usr/bin/env python3
"""
Parse FModel's Game.json localization export to produce data/game_database.json.

Usage:
    python scripts/build_database.py <path_to_Game.json>
    python scripts/build_database.py  # auto-tries common FModel export locations

Run this after each Marvel Rivals game update to refresh the database.
"""

import json
import re
import sys
from pathlib import Path

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

REPO_ROOT   = Path(__file__).resolve().parent.parent
OUT_PATH    = REPO_ROOT / "data" / "game_database.json"

DEFAULT_GAME_JSON_PATHS = [
    Path.home() / "Desktop/MarvelModding/FModel/Output/Exports/Marvel/Content/Localization/Game/en/Game.json",
    Path.home() / "AppData/Local/FModel/Output/Exports/Marvel/Content/Localization/Game/en/Game.json",
]

# ---------------------------------------------------------------------------
# Manual overrides — applied AFTER auto-detection.
# Use these to fix names that the localization stores oddly.
# Key: char_id (str), Value: display name
# ---------------------------------------------------------------------------
CHAR_NAME_OVERRIDES: dict[str, str] = {
    # Add entries here if auto-detection produces a wrong display name, e.g.:
    # "1055": "Daredevil",
}

# ---------------------------------------------------------------------------
# Skin blacklist — these are cosmetic colour variants sharing the same look.
# Entries are substrings matched against skin names (case-insensitive).
# ---------------------------------------------------------------------------
SKIN_NAME_BLACKLIST = ["Cosmic Invasion"]

# ---------------------------------------------------------------------------
# Hardcoded skins not yet in the localization at build time.
# Tuple: (char_id, skin_id, skin_name)
# ---------------------------------------------------------------------------
HARDCODED_SKINS: list[tuple[str, str, str]] = [
    # ("1055", "1055800", "Born Again Season 2"),  # add if missing from Game.json
]


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _title(s: str) -> str:
    """Smarter title-case that preserves common abbreviations."""
    # e.g. "ANTI-VENOM" -> "Anti-Venom", "NYX WEAVER" -> "Nyx Weaver"
    return " ".join(
        "-".join(w.capitalize() for w in part.split("-"))
        for part in s.split()
    )


def _flatten(data: dict) -> dict[str, str]:
    """Flatten {namespace: {key: value}} → {key: value}.
    Duplicate keys are overwritten (last wins) — that's fine for our use."""
    flat: dict[str, str] = {}
    for ns, entries in data.items():
        if isinstance(entries, dict):
            for k, v in entries.items():
                if isinstance(v, str):
                    flat[k] = v
    return flat


# ---------------------------------------------------------------------------
# Extraction helpers
# ---------------------------------------------------------------------------

_RE_SKIN_NAME    = re.compile(r"^UISkinTable_(\d+)_SkinBasic_SkinName$")
_RE_SKIN_NAME2   = re.compile(r"^HeroUIAssetBPTable_(\d{8})_SkinInfo_SkinName$")
_RE_SKIN_ITEM    = re.compile(r"^MarvelItemTable_(\d{7})_ItemName$")  # newer heroes
_RE_HERO_TNAME1  = re.compile(r"^HeroUIAssetBPTable_(\d{8})_HeroInfo_TName$")
_RE_HERO_TNAME2  = re.compile(r"^UIHeroTable_(\d{5})_HeroBasic_TName$")
_RE_HERO_ITEM    = re.compile(r"^MarvelItemTable_(\d{4})_ItemName$")
_RE_NAMEPLATE    = re.compile(r"^UIHeroNameplateTable_3(\d{4})001_Name$")


def extract_skin_names(flat: dict[str, str]) -> dict[str, str]:
    """Return skin_id (7-digit str) → display name."""
    result: dict[str, str] = {}
    for key, val in flat.items():
        # Pattern 1: UISkinTable (newer / seasonal skins) — highest priority
        m = _RE_SKIN_NAME.match(key)
        if m:
            loc_id  = int(m.group(1))
            skin_id = str(loc_id // 10)
            result[skin_id] = _title(val)
            continue
        # Pattern 2: HeroUIAssetBPTable SkinInfo (older / launch skins)
        m = _RE_SKIN_NAME2.match(key)
        if m:
            loc_id  = int(m.group(1))
            skin_id = str(loc_id // 10)
            result.setdefault(skin_id, _title(val))   # don't overwrite UISkinTable
            continue
        # Pattern 3: MarvelItemTable_{7digit}_ItemName
        # Used by newer/season heroes (e.g. Daredevil, Angela, Gambit …)
        # Only hero skin IDs start with "10" in this table.
        m = _RE_SKIN_ITEM.match(key)
        if m:
            skin_id = m.group(1)
            if skin_id[:2] == "10":          # hero skins only
                result.setdefault(skin_id, _title(val))
    return result


def extract_char_names(flat: dict[str, str]) -> dict[str, str]:
    """Return char_id (4-digit str) → display name, trying multiple key patterns."""
    result: dict[str, str] = {}

    # Pattern 1: HeroUIAssetBPTable (older heroes)
    for key, val in flat.items():
        m = _RE_HERO_TNAME1.match(key)
        if m:
            char_id = m.group(1)[:4]
            result.setdefault(char_id, _title(val))

    # Pattern 2: UIHeroTable (newer heroes, season 1+)
    for key, val in flat.items():
        m = _RE_HERO_TNAME2.match(key)
        if m:
            char_id = str(int(m.group(1)) // 10)
            result.setdefault(char_id, _title(val))

    # Pattern 3: UIHeroNameplateTable (most reliable display name)
    for key, val in flat.items():
        m = _RE_NAMEPLATE.match(key)
        if m:
            char_id = m.group(1)
            result[char_id] = _title(val)  # overwrite — nameplate is authoritative

    # Pattern 4: MarvelItemTable_{char_id}_ItemName
    for key, val in flat.items():
        m = _RE_HERO_ITEM.match(key)
        if m:
            char_id = m.group(1)
            # Only use this as fallback if no name found yet
            result.setdefault(char_id, _title(val))

    # Manual overrides (highest priority)
    for char_id, name in CHAR_NAME_OVERRIDES.items():
        result[char_id] = name

    return result


# ---------------------------------------------------------------------------
# Database builder
# ---------------------------------------------------------------------------

def build(game_json_path: Path) -> dict:
    print(f"Reading {game_json_path} …")
    raw = json.loads(game_json_path.read_text(encoding="utf-8"))
    flat = _flatten(raw)
    print(f"  {len(flat):,} total localization keys")

    skin_names  = extract_skin_names(flat)
    char_names  = extract_char_names(flat)

    print(f"  {len(skin_names):,} skin name entries")
    print(f"  {len(char_names):,} char name entries")

    # Group skin_ids by char_id (first 4 digits of skin_id)
    skins_by_char: dict[str, list[dict]] = {}
    for skin_id, skin_name in skin_names.items():
        if len(skin_id) < 4:
            continue
        char_id = skin_id[:4]
        entry = {"skin_id": skin_id, "skin_name": skin_name}
        skins_by_char.setdefault(char_id, []).append(entry)

    # Inject hardcoded extras
    for char_id, skin_id, skin_name in HARDCODED_SKINS:
        entry = {"skin_id": skin_id, "skin_name": skin_name}
        existing_ids = {s["skin_id"] for s in skins_by_char.get(char_id, [])}
        if skin_id not in existing_ids:
            skins_by_char.setdefault(char_id, []).append(entry)

    # Build final characters dict — only include char_ids that have both a name
    # and at least one skin entry.
    # Playable hero char_ids all start with "10" (e.g. 1011, 1035, 1055).
    # Exclude NPCs / bots (4xxx, 9999, etc.).
    characters: dict[str, dict] = {}
    skipped = 0
    for char_id, skins in skins_by_char.items():
        if not (len(char_id) == 4 and char_id.startswith("10")):
            continue   # skip NPCs / bots
        name = char_names.get(char_id)
        if not name:
            skipped += 1
            continue

        default_skin_id = f"{char_id}001"

        # Sort skins: default first, then by skin_id
        skins_sorted = sorted(skins, key=lambda s: (s["skin_id"] != default_skin_id, s["skin_id"]))

        # Apply blacklist
        skins_sorted = [
            s for s in skins_sorted
            if not any(bl.lower() in s["skin_name"].lower() for bl in SKIN_NAME_BLACKLIST)
        ]

        characters[name] = {
            "char_id":        char_id,
            "default_skin_id": default_skin_id,
            "skins":          skins_sorted,
        }

    if skipped:
        print(f"  Skipped {skipped} char_id(s) with no name mapping "
              f"(add to CHAR_NAME_OVERRIDES if needed)")

    # Merge duplicate hero names (e.g. Cloak & Dagger appear under multiple char_ids)
    # Already handled by CHAR_NAME_OVERRIDES mapping both to the same name —
    # last write wins; the skins get merged here.
    deduped: dict[str, dict] = {}
    for name, data in characters.items():
        if name in deduped:
            # Merge skins lists, deduplicating by skin_id
            existing_ids = {s["skin_id"] for s in deduped[name]["skins"]}
            for s in data["skins"]:
                if s["skin_id"] not in existing_ids:
                    deduped[name]["skins"].append(s)
        else:
            deduped[name] = data

    print(f"  {len(deduped)} characters in database")

    return {
        "version": 1,
        "characters": dict(sorted(deduped.items())),
    }


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main() -> None:
    if len(sys.argv) >= 2:
        game_json = Path(sys.argv[1])
    else:
        game_json = None
        for p in DEFAULT_GAME_JSON_PATHS:
            if p.exists():
                game_json = p
                break
        if not game_json:
            print("ERROR: Game.json not found. Pass its path as the first argument.")
            print("  python scripts/build_database.py <path_to_Game.json>")
            sys.exit(1)

    if not game_json.exists():
        print(f"ERROR: File not found: {game_json}")
        sys.exit(1)

    db = build(game_json)

    OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    OUT_PATH.write_text(json.dumps(db, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"\nWrote {OUT_PATH}")
    print(f"  {len(db['characters'])} characters, "
          f"{sum(len(c['skins']) for c in db['characters'].values())} total skins")


if __name__ == "__main__":
    main()
