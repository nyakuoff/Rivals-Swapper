"""
Settings manager for MR-SkinChanger.

Persists user preferences (paths, last selection, active swaps) to a
JSON file.
"""

import json
from pathlib import Path
from dataclasses import dataclass, field, asdict
from typing import Optional


DEFAULT_CONFIG_PATH = Path(__file__).resolve().parent.parent / "config" / "settings.json"


@dataclass
class SwapRecord:
    """Persisted record of a single active skin swap."""
    skin_id: str            # e.g. "1035101"
    skin_name: str          # e.g. "Anti-Venom"
    mod_name: str           # e.g. "Venom_Anti-Venom"
    # Absolute paths to the deployed/output mod files
    pak_path: str = ""
    utoc_path: str = ""
    ucas_path: str = ""


@dataclass
class Settings:
    game_paks_dir: str = ""          # .../Marvel/Content/Paks
    last_character: str = ""         # Last selected character name
    last_skin: str = ""              # Last selected skin ID
    auto_deploy: bool = True         # Auto-deploy to ~mods after packing
    game_content_dir: str = ""       # Extracted game content for file copying
    api_key: str = ""                # MarvelRivalsAPI.com API key for images
    # Active swaps: char_id -> SwapRecord
    active_swaps: dict[str, dict] = field(default_factory=dict)

    def get_swap(self, char_id: str) -> Optional[SwapRecord]:
        """Return the SwapRecord for a character, or None."""
        data = self.active_swaps.get(char_id)
        if data:
            return SwapRecord(**{k: v for k, v in data.items()
                                 if k in SwapRecord.__dataclass_fields__})
        return None

    def set_swap(self, char_id: str, record: SwapRecord) -> None:
        """Register an active swap for a character (one per hero)."""
        self.active_swaps[char_id] = asdict(record)

    def clear_swap(self, char_id: str) -> None:
        """Remove the active swap for a character."""
        self.active_swaps.pop(char_id, None)


def load_settings(path: Path | str | None = None) -> Settings:
    """Load settings from disk. Returns defaults if file missing."""
    cfg_path = Path(path) if path else DEFAULT_CONFIG_PATH
    if not cfg_path.exists():
        return Settings()
    try:
        data = json.loads(cfg_path.read_text(encoding="utf-8"))
        return Settings(**{k: v for k, v in data.items()
                           if k in Settings.__dataclass_fields__})
    except Exception:
        return Settings()


def save_settings(settings: Settings, path: Path | str | None = None) -> None:
    """Persist settings to disk."""
    cfg_path = Path(path) if path else DEFAULT_CONFIG_PATH
    cfg_path.parent.mkdir(parents=True, exist_ok=True)
    cfg_path.write_text(
        json.dumps(asdict(settings), indent=2),
        encoding="utf-8",
    )
