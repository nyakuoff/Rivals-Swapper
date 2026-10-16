"""
Project Galacta integration: downloads the core mod and Companion App from
the official GitHub releases into tools/galacta, installs the mod into ~mods,
and launches the Companion (which rescans ~mods on startup).
"""

import json
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
import zipfile
from pathlib import Path
from typing import Callable

from ._paths import TOOLS_DIR

GALACTA_DIR = TOOLS_DIR / "galacta"
MOD_DIR = GALACTA_DIR / "mod"
COMPANION_DIR = GALACTA_DIR / "companion"
COMPANION_EXE = COMPANION_DIR / "GalactaCompanion.exe"
_OODLE_DLL = TOOLS_DIR / "uassettool" / "oo2core_9_win64.dll"
_RELEASES_API = "https://api.github.com/repos/0xSaturno/ProjectGalacta/releases"
_UA = {"User-Agent": "RivalsSwapper"}


def _latest_assets() -> dict[str, tuple[str, str]]:
    """{"mod"|"companion": (tag, download_url)} from the newest release carrying each."""
    req = urllib.request.Request(_RELEASES_API, headers=_UA)
    with urllib.request.urlopen(req, timeout=10) as resp:
        releases = json.loads(resp.read())
    found: dict[str, tuple[str, str]] = {}
    for rel in releases:  # newest first
        if rel.get("draft") or rel.get("prerelease"):
            continue
        for asset in rel.get("assets", []):
            name = asset["name"]
            if not name.endswith(".zip"):
                continue
            kind = ("companion" if name.startswith("GalactaCompanion")
                    else "mod" if name.startswith("ProjectGalacta") else None)
            if kind and kind not in found:
                found[kind] = (rel["tag_name"], asset["browser_download_url"])
    return found


def _download(tag: str, url: str, dest: Path) -> bool:
    """Extract the release zip into dest unless that tag is already there."""
    version_file = dest / ".version"
    if version_file.is_file() and version_file.read_text().strip() == tag:
        return False
    with tempfile.TemporaryDirectory() as tmp:
        zip_path = Path(tmp) / "asset.zip"
        req = urllib.request.Request(url, headers=_UA)
        with urllib.request.urlopen(req, timeout=120) as resp, open(zip_path, "wb") as f:
            shutil.copyfileobj(resp, f)
        if dest.exists():
            shutil.rmtree(dest)
        dest.mkdir(parents=True)
        with zipfile.ZipFile(zip_path) as z:
            z.extractall(dest)
    version_file.write_text(tag)
    return True


def ensure_galacta(game_paks_dir: Path, log: Callable[[str], None]) -> None:
    """
    Download/update Project Galacta into tools/galacta and make sure the core
    mod in ~mods matches it. Falls back to the cached copy when offline.
    Raises RuntimeError if Galacta isn't available at all.
    """
    try:
        assets = _latest_assets()
    except (OSError, ValueError) as exc:
        assets = {}
        log(f"  [WARN] Couldn't check Project Galacta releases ({exc}); using cached copy.")

    for kind, dest in (("mod", MOD_DIR), ("companion", COMPANION_DIR)):
        if kind in assets:
            tag, url = assets[kind]
            try:
                if _download(tag, url, dest):
                    log(f"  Downloaded Project Galacta {kind} {tag}")
            except (OSError, zipfile.BadZipFile) as exc:
                log(f"  [WARN] Project Galacta {kind} download failed: {exc}")

    # The Companion needs Oodle to read compressed mod containers.
    if COMPANION_EXE.is_file() and not (COMPANION_DIR / _OODLE_DLL.name).exists():
        shutil.copy2(_OODLE_DLL, COMPANION_DIR / _OODLE_DLL.name)

    mod_files = [f for f in MOD_DIR.rglob("*ProjectGalacta*") if f.is_file()]
    if not mod_files:
        raise RuntimeError("Project Galacta couldn't be downloaded. Check your connection and try again.")

    mods_dir = game_paks_dir / "~mods"
    mods_dir.mkdir(parents=True, exist_ok=True)
    for src in mod_files:
        dst = mods_dir / src.name
        if not dst.exists() or dst.read_bytes() != src.read_bytes():
            shutil.copy2(src, dst)
            log(f"  Installed {src.name} to ~mods")


def manifest_path(game_paks_dir: Path) -> Path:
    """skins.txt, written by the Companion and read by the in-game Skin Swapper."""
    return game_paks_dir.parent / "Marvel" / "ProjectGalacta" / "skins.txt"


def manifest_is_current(game_paks_dir: Path) -> bool:
    """True if skins.txt was written after the last change to ~mods."""
    manifest = manifest_path(game_paks_dir)
    mods_dir = game_paks_dir / "~mods"
    if not manifest.is_file():
        return False
    # The dir mtime catches added/removed mods, file mtimes catch overwrites.
    latest = max([mods_dir.stat().st_mtime] + [f.stat().st_mtime for f in mods_dir.iterdir()])
    return manifest.stat().st_mtime >= latest


def wait_for_manifest_write(game_paks_dir: Path, since: float, timeout: float = 900) -> bool:
    """Wait until the Companion rewrites skins.txt. False if it closes first or times out."""
    manifest = manifest_path(game_paks_dir)
    deadline = time.monotonic() + timeout
    time.sleep(5)  # give Wine time to start the process
    while time.monotonic() < deadline:
        if manifest.is_file() and manifest.stat().st_mtime > since:
            return True
        if not _is_running(COMPANION_EXE.name):
            return False
        time.sleep(2)
    return False


def _is_running(image: str) -> bool:
    if sys.platform == "win32":
        out = subprocess.run(
            ["tasklist", "/FI", f"IMAGENAME eq {image}", "/NH"],
            capture_output=True, text=True, creationflags=subprocess.CREATE_NO_WINDOW,
        ).stdout
        return image.lower() in out.lower()
    # Anchor to argv[0] so shells/editors merely mentioning the path don't match.
    pattern = rf"^(\S*[\\/])?{re.escape(image)}( |$)"
    return subprocess.run(["pgrep", "-f", pattern], capture_output=True).returncode == 0


def launch_companion() -> str:
    """Start the Companion (via Wine off Windows). Returns a message for the log."""
    exe = COMPANION_EXE
    if not exe.is_file():
        return "[WARN] Galacta Companion isn't downloaded yet."
    if _is_running(exe.name):
        return ("Galacta Companion is already open: click Scan Mods, "
                "then Write in its Manifest tab.")

    if sys.platform == "win32":
        subprocess.Popen([str(exe)], cwd=exe.parent,
                         creationflags=subprocess.DETACHED_PROCESS)
    else:
        if shutil.which("wine") is None:
            return "[WARN] Wine isn't installed, so the Galacta Companion can't be launched."
        subprocess.Popen(["wine", str(exe)], cwd=exe.parent,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                         start_new_session=True)
    return ("Opened Galacta Companion. It scans your mods on launch; "
            "click Write in its Manifest tab to apply.")
