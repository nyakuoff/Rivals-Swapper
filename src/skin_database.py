"""
Skin database for MR-SkinChanger.

Provides character metadata and skin IDs.
Supports both hardcoded data and fetching from the natimerry API.
"""

import requests
from dataclasses import dataclass, field
from typing import Optional


API_URL = "https://rivals.natimerry.com/skins"

# Skin names containing any of these substrings are hidden from the UI
SKIN_BLACKLIST = ["Cosmic Invasion"]

# Hardcoded skins that are not yet in the API.
# Each entry: (character_name, skin_id, skin_name)
_HARDCODED_SKINS: list[tuple[str, str, str]] = [
    ("Daredevil", "1055800", "Born Again Season 2"),
]


@dataclass
class SkinInfo:
    """A single skin entry."""
    skin_id: str        # e.g. "1055500"
    skin_name: str      # e.g. "Devil 2099"
    wiki_url: str = ""  # Optional wiki/image URL


@dataclass
class CharacterInfo:
    """Full character entry with skins."""
    name: str
    char_id: str                          # e.g. "1055"
    default_skin_id: str                  # e.g. "1055001"
    skins: list[SkinInfo] = field(default_factory=list)


class SkinDatabase:
    """
    Manages the skin/character database.
    Fetches all data from the natimerry API.
    """

    def __init__(self) -> None:
        self.characters: dict[str, CharacterInfo] = {}

    # ------------------------------------------------------------------
    # API fetch
    # ------------------------------------------------------------------

    def fetch_from_api(self) -> bool:
        """
        Fetch the latest skin data from the natimerry API and merge it
        into the local database.  Returns True on success.
        """
        try:
            resp = requests.get(API_URL, timeout=10)
            resp.raise_for_status()
            data: list[dict] = resp.json()
        except Exception as exc:
            print(f"[SkinDB] API fetch failed: {exc}")
            return False

        # Group entries by character name
        grouped: dict[str, list[dict]] = {}
        for entry in data:
            name = entry.get("name", "")
            if name:
                grouped.setdefault(name, []).append(entry)

        for name, entries in grouped.items():
            # Determine character ID from any entry's "id" field
            # The "id" field is the default skin id (charID + "001")
            any_id = entries[0].get("id", "")
            if not any_id:
                continue
            char_id = any_id[:-3]  # strip trailing "001" suffix → character ID

            default_skin_id = f"{char_id}001"

            skins: list[SkinInfo] = []
            seen_skin_ids: set[str] = set()
            for entry in entries:
                skin_id = entry.get("skinid") or entry.get("id", "")
                skin_name = entry.get("skin_name", "Unknown")
                url = entry.get("url", "")
                # Skip duplicate skin IDs — these are colour options
                # (variants) of the same base skin and share all assets.
                if skin_id and skin_id not in seen_skin_ids:
                    seen_skin_ids.add(skin_id)
                    skins.append(SkinInfo(str(skin_id), skin_name, url))

            if name in self.characters:
                # Merge: update skins list
                existing = self.characters[name]
                existing.skins = skins
            else:
                self.characters[name] = CharacterInfo(
                    name=name,
                    char_id=char_id,
                    default_skin_id=default_skin_id,
                    skins=skins,
                )

        print(f"[SkinDB] Loaded {len(self.characters)} characters from API")
        self._apply_hardcoded_skins()
        return True

    def _apply_hardcoded_skins(self) -> None:
        """Inject skins that are not yet in the API into the database."""
        for char_name, skin_id, skin_name in _HARDCODED_SKINS:
            char = self.characters.get(char_name)
            if char is None:
                continue
            # Only add if not already present (API may catch up later)
            existing_ids = {s.skin_id for s in char.skins}
            if skin_id not in existing_ids:
                char.skins.append(SkinInfo(skin_id, skin_name))
                print(f"[SkinDB] Injected hardcoded skin: {char_name} / {skin_id} ({skin_name})")

    # ------------------------------------------------------------------
    # Queries
    # ------------------------------------------------------------------

    def get_character_names(self) -> list[str]:
        """Return sorted list of character names."""
        return sorted(self.characters.keys())

    def get_character(self, name: str) -> Optional[CharacterInfo]:
        """Look up a character by name."""
        return self.characters.get(name)

    def get_skins(self, character_name: str) -> list[SkinInfo]:
        """Return list of skins for a character (excluding default and blacklisted)."""
        char = self.characters.get(character_name)
        if not char:
            return []
        return [
            s for s in char.skins
            if s.skin_id != char.default_skin_id
            and not any(bl.lower() in s.skin_name.lower() for bl in SKIN_BLACKLIST)
        ]

    def get_all_skins(self, character_name: str) -> list[SkinInfo]:
        """Return list of ALL skins including default."""
        char = self.characters.get(character_name)
        if not char:
            return []
        return list(char.skins)
