"""
umodel wrapper for MR-SkinChanger.

Exports skin icon and hero portrait textures from Marvel Rivals .pak files
using umodel_64.exe, writing .tga files to an output directory that
ImageCache.populate_from_umodel() then converts into the disk cache.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path
from typing import Callable, Optional

from ._paths import TOOLS_DIR, PROJECT_ROOT

UMODEL_EXE   = TOOLS_DIR / "umodel" / "umodel_materials_ue5.exe"
_AES_KEY     = "0C263D8C22DCB085894899C3A3796383E9BF9DE0CBFB08C9BF2DEF2E84F29D74"
_UMODEL_GAME = "marv"
UMODEL_OUT   = PROJECT_ROOT / "data" / "umodel_out"

# Hide console window when launching subprocesses on Windows
_SUBPROCESS_FLAGS = subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0

def _to_wine_path(path: Path) -> str:
    """Convert an absolute Linux path to a Wine Windows path (Z:\\...)."""
    return "Z:" + str(path).replace("/", "\\")


# umodel package masks — these match any .uasset whose name starts with the prefix
_EXPORT_MASKS = [
    "img_skin_*",
    "img_heroportrait_*",
]


class UModelWrapper:
    """Wraps umodel_64.exe to export skin/portrait textures from game paks."""

    def __init__(
        self,
        game_paks_dir: str | Path,
        output_dir: str | Path = UMODEL_OUT,
    ) -> None:
        self.game_paks_dir = Path(game_paks_dir)
        self.output_dir    = Path(output_dir)

    # ------------------------------------------------------------------
    # Public interface
    # ------------------------------------------------------------------

    def validate(self) -> list[str]:
        """
        Return a list of human-readable problems.
        An empty list means we're ready to run.
        """
        problems: list[str] = []
        if not UMODEL_EXE.exists():
            problems.append(f"umodel not found: {UMODEL_EXE}")
        if sys.platform != "win32" and shutil.which("wine") is None:
            problems.append(
                "wine is not installed or not in PATH.\n"
                "Install Wine to run umodel on Linux (e.g. sudo dnf install wine)."
            )
        if not self.game_paks_dir.exists():
            problems.append(f"Game Paks folder not found: {self.game_paks_dir}")
        elif not any(self.game_paks_dir.glob("*.pak")):
            problems.append(
                f"No .pak files found in: {self.game_paks_dir}\n"
                "Make sure the Game Paks Folder is set correctly in Settings."
            )
        return problems

    def export_skin_images(
        self,
        progress_cb: Optional[Callable[[str], None]] = None,
    ) -> bool:
        """
        Run umodel to export skin icons and hero portraits.

        umodel only accepts one package-wildcard per invocation, so we run
        it once per mask and consider the overall result successful when at
        least one run succeeds.

        progress_cb(message) is called with status strings during the run.
        Returns True if all umodel invocations exited successfully (rc == 0).
        """
        self.output_dir.mkdir(parents=True, exist_ok=True)

        # On Linux, run the Windows .exe through Wine and convert paths
        if sys.platform == "win32":
            exe_prefix = []
            paks_path  = str(self.game_paks_dir)
            out_path   = str(self.output_dir)
        else:
            exe_prefix = ["wine"]
            paks_path  = _to_wine_path(self.game_paks_dir)
            out_path   = _to_wine_path(self.output_dir)

        base_cmd = [
            *exe_prefix,
            str(UMODEL_EXE),
            "-export",
            f"-game={_UMODEL_GAME}",
            f"-path={paks_path}",
            f"-out={out_path}",
            f"-aes=0x{_AES_KEY}",
            "-nooverwrite",      # skip already-exported files
        ]

        all_ok = True
        for i, mask in enumerate(_EXPORT_MASKS, 1):
            cmd = base_cmd + [mask]
            if progress_cb:
                progress_cb(f"[{i}/{len(_EXPORT_MASKS)}] Exporting {mask}…")

            try:
                proc = subprocess.Popen(
                    cmd,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                    creationflags=_SUBPROCESS_FLAGS,
                )
            except OSError as exc:
                print(f"[UModel] Failed to launch umodel: {exc}")
                all_ok = False
                continue

            assert proc.stdout is not None
            for line in proc.stdout:
                line = line.rstrip()
                if line:
                    print(f"[umodel] {line}")
                    if progress_cb:
                        progress_cb(line)

            rc = proc.wait()
            if rc != 0:
                print(f"[UModel] umodel exited with code {rc} for mask {mask!r}")
                all_ok = False

        return all_ok

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    @property
    def tga_files(self) -> list[Path]:
        """All .tga files already present in the output directory."""
        return list(self.output_dir.rglob("*.tga"))

    def cleanup_output(self) -> None:
        """Delete the umodel output directory after TGAs have been cached."""
        if self.output_dir.exists():
            shutil.rmtree(self.output_dir, ignore_errors=True)
            print(f"[UModel] cleaned up {self.output_dir}")
