"""
Image cache for MR-SkinChanger.

Downloads hero portraits and skin costume images from the Marvel Rivals API
(marvelrivalsapi.com) and caches them on disk under data/images/.

Performance:
  - requests.Session for TCP connection reuse.
  - All costumes API calls run in parallel, JSON cached in memory.
  - ALL image downloads (portraits + skin icons) in one parallel batch.
"""

from __future__ import annotations

import re
import threading
from io import BytesIO
from pathlib import Path
from typing import Optional
from concurrent.futures import ThreadPoolExecutor, as_completed

import requests
from PIL import Image

from ._paths import PROJECT_ROOT

API_BASE = "https://marvelrivalsapi.com/api/v1"
COSTUME_CDN = "https://marvelrivalsapi.com/rivals"
CACHE_DIR = PROJECT_ROOT / "data" / "images"
MAX_WORKERS = 16
MAX_CACHED_PX = 256  # pre-resize images on disk so Tk doesn't struggle

# Hardcoded costume entries for skins not yet in the marvelrivalsapi.com API.
# Each entry: (hero_db_name, costume_dict) where costume_dict uses the same
# shape as the API response: {"id": str, "icon": url_or_partial_path, ...}
# Full https:// URLs are passed through as-is by _costume_image_url().
_HARDCODED_COSTUMES: list[tuple[str, dict]] = [
    ("Daredevil", {
        "id": "1055800",
        "icon": "https://static.wikia.nocookie.net/marvel-rivals/images/0/0e/CosInfo_-_Daredevil_Daredevil_Born_Again_Season_2_Icon.png",
    }),
]


def _safe(name: str) -> str:
    return re.sub(r"[^a-zA-Z0-9_-]", "_", name)


class ImageCache:
    """Thread-safe, disk-backed image cache."""

    def __init__(self) -> None:
        CACHE_DIR.mkdir(parents=True, exist_ok=True)
        self._lock = threading.Lock()
        self._mem: dict[str, Optional[Image.Image]] = {}
        self._slug_map: dict[str, str] = {}
        self._costumes_cache: dict[str, list] = {}
        self._session: requests.Session | None = None
        self._repair_cache()

    # -- session -------------------------------------------------------

    def _get_session(self) -> requests.Session:
        if self._session is None:
            s = requests.Session()
            a = requests.adapters.HTTPAdapter(
                pool_connections=MAX_WORKERS,
                pool_maxsize=MAX_WORKERS,
                max_retries=1,
            )
            s.mount("https://", a)
            s.mount("http://", a)
            self._session = s
        return self._session

    # -- cache integrity -------------------------------------------------

    def _repair_cache(self) -> None:
        """Delete any corrupt/unreadable .png files so they get re-downloaded."""
        removed = 0
        for f in CACHE_DIR.glob("*.png"):
            try:
                img = Image.open(f)
                img.verify()  # fast integrity check, does not decode fully
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

    def get_api_hero_names(self) -> set[str]:
        """Return the set of DB hero names that have a known API slug."""
        with self._lock:
            return set(self._slug_map.keys())

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
                # Corrupt file — remove so it gets re-downloaded
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
        # Pre-resize to max cached size to keep disk + render fast
        img = self._fit(img, MAX_CACHED_PX)
        with self._lock:
            self._mem[key] = img
        disk = CACHE_DIR / f"{key}.png"
        tmp = disk.with_suffix(".tmp")
        try:
            img.save(tmp, "PNG")
            tmp.replace(disk)  # atomic on Windows NTFS
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
        # Verify the file is actually readable
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

    # -- url helpers ---------------------------------------------------

    @staticmethod
    def _headers(api_key: str) -> dict[str, str]:
        return {"x-api-key": api_key, "Accept": "application/json"}

    @staticmethod
    def _costume_image_url(partial: str) -> str:
        if not partial:
            return ""
        if partial.startswith("http"):
            return partial
        if not partial.startswith("/"):
            partial = "/" + partial
        return COSTUME_CDN + partial

    def _download_image(self, url: str) -> Optional[Image.Image]:
        if not url:
            return None
        try:
            r = self._get_session().get(url, timeout=15)
            r.raise_for_status()
            return Image.open(BytesIO(r.content)).convert("RGBA")
        except Exception:
            return None

    # -- Phase 1: slug map (1 API call) --------------------------------

    def build_slug_map(self, api_key: str, hero_names: list[str]) -> None:
        headers = self._headers(api_key)
        session = self._get_session()

        # --- Pass 1: match from /heroes list ---
        try:
            r = session.get(
                f"{API_BASE}/heroes",
                headers=headers,
                timeout=15,
            )
            r.raise_for_status()
            api_heroes = r.json()
        except Exception as exc:
            print(f"[ImageCache] heroes API failed: {exc}")
            api_heroes = []

        lower_map = {n.lower(): n for n in hero_names}
        for h in api_heroes:
            slug = h.get("name", "")
            real = h.get("real_name", "")
            matched = lower_map.get(slug.lower())
            if not matched and real:
                matched = lower_map.get(real.lower())
            if matched:
                with self._lock:
                    self._slug_map[matched] = slug

        # --- Pass 2: probe unmatched heroes directly via costumes endpoint ---
        with self._lock:
            matched_names = set(self._slug_map.keys())
        unmatched = [n for n in hero_names if n not in matched_names]

        if unmatched:
            def _probe(name: str) -> None:
                slug = name.lower()
                try:
                    resp = session.get(
                        f"{API_BASE}/heroes/hero/{slug}/costumes",
                        headers=headers,
                        timeout=10,
                    )
                    if resp.status_code == 200 and resp.json():
                        with self._lock:
                            self._slug_map[name] = slug
                except Exception:
                    pass

            with ThreadPoolExecutor(max_workers=MAX_WORKERS) as pool:
                list(pool.map(_probe, unmatched))

    # -- Phase 2: fetch all costumes JSON in parallel ------------------

    def fetch_all_costumes(
        self,
        api_key: str,
        hero_names: list[str],
        progress_callback: Optional[callable] = None,
    ) -> None:
        headers = self._headers(api_key)
        session = self._get_session()

        work: list[tuple[str, str]] = []
        for name in hero_names:
            with self._lock:
                slug = self._slug_map.get(name, "")
            if slug:
                work.append((name, slug))

        total = len(work)
        done_count = 0
        clock = threading.Lock()

        def _fetch(item: tuple[str, str]) -> None:
            nonlocal done_count
            db_name, slug = item
            try:
                resp = session.get(
                    f"{API_BASE}/heroes/hero/{slug.replace(' ', '%20')}/costumes",
                    headers=headers,
                    timeout=15,
                )
                resp.raise_for_status()
                data = resp.json()
                with self._lock:
                    self._costumes_cache[db_name] = data
            except Exception:
                pass
            with clock:
                done_count += 1
                d = done_count
            if progress_callback:
                progress_callback(d, total)

        with ThreadPoolExecutor(max_workers=MAX_WORKERS) as pool:
            futs = [pool.submit(_fetch, w) for w in work]
            for f in as_completed(futs):
                try:
                    f.result()
                except Exception:
                    pass

    # -- Phase 3: inject hardcoded costumes ---------------------------

    def inject_hardcoded_costumes(self) -> None:
        """
        Merge hardcoded costume entries (for skins not yet in the API)
        into the in-memory costumes cache so that collect_download_tasks
        will find their icon URLs and skin_database entries will have images.
        """
        with self._lock:
            for hero_name, costume in _HARDCODED_COSTUMES:
                if hero_name not in self._costumes_cache:
                    self._costumes_cache[hero_name] = []
                costumes = self._costumes_cache[hero_name]
                existing_ids = {str(c.get("id", "")) for c in costumes}
                cid = str(costume.get("id", ""))
                if cid and cid not in existing_ids:
                    costumes.append(costume)

    # -- Phase 4: collect download tasks from cached costumes ----------

    def collect_download_tasks(
        self,
        heroes: list[tuple[str, str]],
        skin_ids_by_hero: dict[str, list[str]],
    ) -> list[tuple[str, str | list[str]]]:
        """Return (cache_key, url_or_urls) pairs for images that need downloading.
        
        For portrait keys, url may be a list of fallback URLs (tried in order).
        For skin keys, url is always a single string.
        """
        tasks: list[tuple[str, str | list[str]]] = []
        default_ids = {n: s for n, s in heroes}

        with self._lock:
            cache = dict(self._costumes_cache)

        for db_name, costumes in cache.items():
            dsid = default_ids.get(db_name, "")
            wanted = set(skin_ids_by_hero.get(db_name, []))

            pkey = self.hero_portrait_key(db_name)
            need_portrait = not self.has(pkey)
            # Collect all valid icon URLs for portrait fallback
            all_icon_urls: list[str] = []
            default_url: str = ""

            for c in costumes:
                cid = str(c.get("id", ""))
                icon = c.get("icon", "")
                if not icon:
                    continue
                url = self._costume_image_url(icon)
                all_icon_urls.append(url)

                if cid == dsid:
                    default_url = url

                if cid in wanted:
                    skey = self.skin_icon_key(db_name, cid)
                    if not self.has(skey):
                        tasks.append((skey, url))

            # Portrait: try default URL first, then all others as fallbacks
            if need_portrait and all_icon_urls:
                if default_url:
                    # Put default first, then the rest as fallbacks
                    fallbacks = [default_url] + [u for u in all_icon_urls if u != default_url]
                else:
                    fallbacks = all_icon_urls
                tasks.append((pkey, fallbacks))

        return tasks

    # -- Phase 4: parallel image download ------------------------------

    def batch_download_images(
        self,
        tasks: list[tuple[str, str | list[str]]],
        progress_callback: Optional[callable] = None,
    ) -> None:
        if not tasks:
            return

        total = len(tasks)
        done_count = 0
        clock = threading.Lock()

        def _do(item: tuple[str, str | list[str]]) -> None:
            nonlocal done_count
            key, url_or_urls = item
            # Support single URL string or list of fallback URLs
            urls = url_or_urls if isinstance(url_or_urls, list) else [url_or_urls]
            img = None
            for url in urls:
                img = self._download_image(url)
                if img:
                    break
            if img:
                self.put(key, img)
            with clock:
                done_count += 1
                d = done_count
            if progress_callback:
                progress_callback(d, total)

        with ThreadPoolExecutor(max_workers=MAX_WORKERS) as pool:
            futs = [pool.submit(_do, t) for t in tasks]
            for f in as_completed(futs):
                try:
                    f.result()
                except Exception:
                    pass
