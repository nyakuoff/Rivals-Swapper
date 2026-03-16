"""
Image cache for MR-SkinChanger.

Caches hero portraits and skin icons on disk under data/images/.
Images are sourced from umodel texture exports (no network calls).
"""

from __future__ import annotations

import re
import threading
from pathlib import Path
from typing import Callable, Optional

from PIL import Image

from ._paths import PROJECT_ROOT

CACHE_DIR    = PROJECT_ROOT / "data" / "images"
MAX_CACHED_PX = 256   # pre-resize images on disk so Tk doesn't struggle

# umodel mirrors the UE content path inside the export dir.
# Skin icons:   <out>/Marvel/Content/.../Show/Skin/img_skin_{skin_id}.tga
# Hero portraits: <out>/Marvel/Content/.../Show/Skin/OriginalSkin/img_heroportrait_{char_id}0010_portrait.tga
_RE_SKIN_ICON   = re.compile(r"img_skin_(\d{7})\.tga$", re.IGNORECASE)
_RE_PORTRAIT    = re.compile(r"img_heroportrait_(\d{4})\d{4}_portrait\.tga$", re.IGNORECASE)


def _safe(name: str) -> str:
    return re.sub(r"[^a-zA-Z0-9_-]", "_", name)


class ImageCache:
    """Thread-safe, disk-backed image cache."""

    def __init__(self) -> None:
        CACHE_DIR.mkdir(parents=True, exist_ok=True)
        self._lock = threading.Lock()
        self._mem: dict[str, Optional[Image.Image]] = {}
        self._repair_cache()

    # -- cache integrity -------------------------------------------------

    def _repair_cache(self) -> None:
        """Delete any corrupt/unreadable .png files so they get re-cached."""
        removed = 0
        for f in CACHE_DIR.glob("*.png"):
            try:
                img = Image.open(f)
                img.verify()
            except Exception:
                try:
                    f.unlink()
                    removed += 1
                except OSError:
                    pass
        if removed:
            print(f"[ImageCache] removed {removed} corrupt cached image(s)")

    # -- key helpers ---------------------------------------------------

    @staticmethod
    def hero_portrait_key(name: str) -> str:
        return f"portrait__{_safe(name)}"

    @staticmethod
    def skin_icon_key(hero: str, sid: str) -> str:
        return f"skin__{_safe(hero)}__{_safe(sid)}"

    # -- get / put / has -----------------------------------------------

    def get(self, key: str) -> Optional[Image.Image]:
        with self._lock:
            if key in self._mem:
                return self._mem[key]
        disk = CACHE_DIR / f"{key}.png"
        if disk.exists():
            try:
                img = Image.open(disk).copy()
                with self._lock:
                    self._mem[key] = img
                return img
            except Exception:
                try:
                    disk.unlink()
                except OSError:
                    pass
        return None

    @staticmethod
    def _fit(img: Image.Image, max_px: int) -> Image.Image:
        """Downscale img so its longest side is at most max_px."""
        w, h = img.size
        if w <= max_px and h <= max_px:
            return img
        scale = max_px / max(w, h)
        return img.resize((int(w * scale), int(h * scale)), Image.LANCZOS)

    def put(self, key: str, img: Image.Image) -> None:
        img = self._fit(img, MAX_CACHED_PX)
        with self._lock:
            self._mem[key] = img
        disk = CACHE_DIR / f"{key}.png"
        tmp = disk.with_suffix(".tmp")
        try:
            img.save(tmp, "PNG")
            tmp.replace(disk)
        except Exception:
            try:
                tmp.unlink(missing_ok=True)
            except OSError:
                pass

    def has(self, key: str) -> bool:
        with self._lock:
            if key in self._mem:
                return True
        disk = CACHE_DIR / f"{key}.png"
        if not disk.exists():
            return False
        try:
            img = Image.open(disk)
            img.verify()
            return True
        except Exception:
            try:
                disk.unlink()
            except OSError:
                pass
            return False

    # -- umodel populate -----------------------------------------------

    def populate_from_umodel(
        self,
        umodel_out_dir: str | Path,
        char_id_to_name: dict[str, str],
        progress_cb: Optional[Callable[[int, int], None]] = None,
    ) -> int:
        """
        Walk *umodel_out_dir* for exported .tga files and convert them
        into the PNG disk cache.

        char_id_to_name maps 4-digit char_id strings to their display names
        (e.g. {"1022": "Captain America", ...}).

        Returns the number of images successfully cached.
        """
        umodel_out_dir = Path(umodel_out_dir)
        tga_files = list(umodel_out_dir.rglob("*.tga"))
        total   = len(tga_files)
        cached  = 0

        for i, tga in enumerate(tga_files):
            name = tga.name

            # --- hero portrait ---
            m = _RE_PORTRAIT.search(name)
            if m:
                char_id   = m.group(1)
                hero_name = char_id_to_name.get(char_id)
                if hero_name:
                    key = self.hero_portrait_key(hero_name)
                    if not self.has(key):
                        img = self._load_tga(tga)
                        if img:
                            self.put(key, img)
                            cached += 1

            # --- skin icon ---
            m = _RE_SKIN_ICON.search(name)
            if m:
                skin_id   = m.group(1)
                char_id   = skin_id[:4]
                hero_name = char_id_to_name.get(char_id)
                if hero_name:
                    key = self.skin_icon_key(hero_name, skin_id)
                    if not self.has(key):
                        img = self._load_tga(tga)
                        if img:
                            self.put(key, img)
                            cached += 1

            if progress_cb:
                progress_cb(i + 1, total)

        print(f"[ImageCache] populated {cached} images from umodel output")
        return cached

    @staticmethod
    def _load_tga(path: Path) -> Optional[Image.Image]:
        try:
            return Image.open(path).convert("RGBA")
        except Exception as exc:
            print(f"[ImageCache] could not load {path.name}: {exc}")
            return None
