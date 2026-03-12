"""
Wrapper around trumank/retoc for IOStore extraction and packing.

Tools:
    tools/retoc/retoc.exe   — IOStore CLI (list, to-legacy, to-zen)

Pipeline:
    1. extract_skin()       — retoc to-legacy with --filter → legacy .uasset/.uexp/.ubulk
    2. pack_to_iostore()    — retoc to-zen → .pak + .utoc + .ucas
    3. deploy_to_mods()     — copy IOStore files to game's ~mods folder
"""

import os
import shutil
import subprocess
from pathlib import Path
from dataclasses import dataclass
from typing import Optional


PROJECT_ROOT = Path(__file__).resolve().parent.parent
RETOC_EXE = PROJECT_ROOT / "tools" / "retoc" / "retoc.exe"

AES_KEY = "0C263D8C22DCB085894899C3A3796383E9BF9DE0CBFB08C9BF2DEF2E84F29D74"


@dataclass
class ExtractResult:
    success: bool
    output_dir: Optional[Path] = None
    files_extracted: int = 0
    error: str = ""


@dataclass
class PackResult:
    success: bool
    pak_path: Optional[Path] = None
    utoc_path: Optional[Path] = None
    ucas_path: Optional[Path] = None
    error: str = ""


class RetocWrapper:
    """
    Drives retoc to extract game assets and pack mods into IOStore format.

    Expects retoc at: tools/retoc/retoc.exe
    """

    def __init__(
        self,
        output_dir: Path | str,
        game_paks_dir: Optional[Path | str] = None,
    ) -> None:
        self.output_dir = Path(output_dir)
        self.game_paks_dir = Path(game_paks_dir) if game_paks_dir else None
        self.output_dir.mkdir(parents=True, exist_ok=True)

    # ------------------------------------------------------------------
    # Validation
    # ------------------------------------------------------------------

    def validate(self) -> list[str]:
        """Return a list of problems (empty = all good)."""
        problems: list[str] = []
        if not RETOC_EXE.is_file():
            problems.append(f"retoc.exe not found at: {RETOC_EXE}")
        if not self.game_paks_dir or not self.game_paks_dir.is_dir():
            problems.append("Game Paks directory not set or doesn't exist.")
        return problems

    # ------------------------------------------------------------------
    # Game link directory
    # ------------------------------------------------------------------

    # ------------------------------------------------------------------
    # Extract skin meshes
    # ------------------------------------------------------------------

    def _run_extract(
        self,
        filter_pattern: str | list[str],
        output_dir: Path,
    ) -> ExtractResult:
        """
        Low-level helper: run retoc to-legacy with one or more filters.

        Cleans *output_dir* first, then extracts into it.

        Returns:
            ExtractResult with output_dir and file count on success.
        """
        if output_dir.exists():
            shutil.rmtree(output_dir)
        output_dir.mkdir(parents=True)

        if not self.game_paks_dir or not self.game_paks_dir.is_dir():
            return ExtractResult(
                False,
                error="Game Paks directory not configured or doesn't exist",
            )

        # Accept a single string or a list of filter patterns
        filters = [filter_pattern] if isinstance(filter_pattern, str) else filter_pattern

        cmd = [
            str(RETOC_EXE),
            "--aes-key", AES_KEY,
            "to-legacy",
            str(self.game_paks_dir),
            str(output_dir),
        ]
        for f in filters:
            cmd.extend(["--filter", f])
        cmd.extend(["--no-shaders", "-v"])

        try:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=300,
            )
        except FileNotFoundError:
            return ExtractResult(False, error=f"retoc.exe not found at {RETOC_EXE}")
        except subprocess.TimeoutExpired:
            return ExtractResult(False, error="retoc to-legacy timed out (300s)")

        output = (result.stdout + "\n" + result.stderr).strip()

        if result.returncode != 0:
            return ExtractResult(
                False,
                error=f"retoc to-legacy failed (rc={result.returncode}):\n{output}",
            )

        # Count extracted files (exclude scriptobjects.bin)
        extracted_files = []
        for root, dirs, files in os.walk(output_dir):
            for f in files:
                if f != "scriptobjects.bin":
                    extracted_files.append(os.path.join(root, f))

        if not extracted_files:
            return ExtractResult(
                False,
                output_dir=output_dir,
                error=f"retoc ran but no files extracted. Output:\n{output}",
            )

        return ExtractResult(
            True,
            output_dir=output_dir,
            files_extracted=len(extracted_files),
        )

    def extract_skin(
        self,
        char_id: str,
        skin_id: str,
        output_dir: Optional[Path] = None,
    ) -> ExtractResult:
        """
        Extract a skin's mesh, material, and texture files from the game.

        Uses --filter to extract files matching the character/skin
        Meshes, Materials, and Textures paths.  This covers both
        mesh-swap skins (which have a Meshes folder) and retexture-only
        skins (which only have Materials and/or Textures).

        Extracts to legacy .uasset/.uexp/.ubulk format.

        Args:
            char_id:    Character ID (e.g. "1055")
            skin_id:    Skin ID (e.g. "1055500")
            output_dir: Where to place extracted files (default: data/extracted/<skin_id>)

        Returns:
            ExtractResult with output_dir on success.
        """
        if output_dir is None:
            output_dir = PROJECT_ROOT / "data" / "extracted" / skin_id

        # Use "Marvel/Characters/..." prefix so the filter doesn't
        # accidentally match VFX paths like
        #   VFX/Materials/Characters/{charID}/{skinID}/Materials/...
        # which contain the same substring.
        filter_patterns = [
            f"Marvel/Characters/{char_id}/{skin_id}/Meshes",
            f"Marvel/Characters/{char_id}/{skin_id}/Materials",
            f"Marvel/Characters/{char_id}/{skin_id}/Textures",
        ]
        return self._run_extract(filter_patterns, output_dir)

    def extract_skin_weapons(
        self,
        char_id: str,
        skin_id: str,
        output_dir: Optional[Path] = None,
    ) -> ExtractResult:
        """
        Extract a skin's weapon files from the game.

        Filters on ``Characters/{char_id}/{skin_id}/Weapons`` to get
        all weapon data including Meshes, Materials, and Textures
        sub-folders within each weapon directory.

        Args:
            char_id:    Character ID (e.g. "1055")
            skin_id:    Skin ID (e.g. "1055500")
            output_dir: Where to place extracted files
                        (default: data/extracted/{skin_id}_weapons)

        Returns:
            ExtractResult with output_dir on success.
        """
        if output_dir is None:
            output_dir = PROJECT_ROOT / "data" / "extracted" / f"{skin_id}_weapons"

        filter_pattern = f"Characters/{char_id}/{skin_id}/Weapons"
        return self._run_extract(filter_pattern, output_dir)

    def extract_skin_blueprints(
        self,
        char_id: str,
        skin_id: str,
        output_dir: Optional[Path] = None,
    ) -> ExtractResult:
        """
        Extract the skin's ChildBP (and related blueprints).

        The ChildBP is the key asset that drives VFX: it contains
        Niagara particle system references, VFX mesh bindings, and
        weapon slot assignments.  By including the source skin's
        ChildBP in the mod (patched to load at the default path),
        the game will use the source skin's VFX effects.

        Args:
            char_id:    Character ID (e.g. "1055")
            skin_id:    Skin ID (e.g. "1055500")
            output_dir: Where to place extracted files
                        (default: data/extracted/{skin_id}_bp)

        Returns:
            ExtractResult with output_dir on success.
        """
        if output_dir is None:
            output_dir = PROJECT_ROOT / "data" / "extracted" / f"{skin_id}_bp"

        # Extract just the blueprint files by name.
        # The ChildBP is the critical one — it drives VFX and weapon
        # slot assignment for the skin.
        filter_pattern = f"{skin_id}_ChildBP"
        return self._run_extract(filter_pattern, output_dir)

    def extract_skin_vfx(
        self,
        char_id: str,
        skin_id: str,
        output_dir: Optional[Path] = None,
    ) -> ExtractResult:
        """
        Extract ALL VFX asset files for a skin from the game.

        VFX assets are spread across several sub-directories:
        ``Marvel/VFX/{Type}/Characters/{char_id}/{skin_id}/``
        where Type ∈ {Meshes, Particles, Textures, Materials,
        DecalAnimation, VAT}.

        Note: Materials uses an extra nesting level:
        ``VFX/Materials/Characters/{char_id}/Materials/{skin_id}/``

        Args:
            char_id:    Character ID (e.g. "1055")
            skin_id:    Skin ID (e.g. "1055500")
            output_dir: Where to place extracted files
                        (default: data/extracted/{skin_id}_vfx)

        Returns:
            ExtractResult with output_dir on success.
        """
        if output_dir is None:
            output_dir = PROJECT_ROOT / "data" / "extracted" / f"{skin_id}_vfx"

        vfx_types = ["Meshes", "Particles", "Textures",
                      "Materials", "DecalAnimation", "VAT"]
        filter_patterns = [
            f"VFX/{vtype}/Characters/{char_id}/{skin_id}"
            for vtype in vfx_types
        ]
        # Materials uses extra nesting: .../Characters/{charID}/Materials/{skinID}
        filter_patterns.append(
            f"VFX/Materials/Characters/{char_id}/Materials/{skin_id}"
        )
        # Niagara Parameter Collection Instance (NPCI) — skin-specific
        # colour overrides for VFX.  Located at:
        #   VFX/Particles/NiagaraParameterCollection/{charID}/NPCI_{skinID}
        filter_patterns.append(
            f"NiagaraParameterCollection/{char_id}/NPCI_{skin_id}"
        )
        return self._run_extract(filter_patterns, output_dir)

    # ------------------------------------------------------------------
    # Pack to IOStore
    # ------------------------------------------------------------------

    def pack_to_iostore(
        self,
        staging_dir: Path,
        mod_name: str,
    ) -> PackResult:
        """
        Pack a staging directory into IOStore format using retoc to-zen.

        The staging_dir should contain the Marvel/Content/... tree with
        the renamed skin files. A scriptobjects.bin should also be present
        at the staging root.

        Args:
            staging_dir: Folder containing Marvel/ tree + scriptobjects.bin
            mod_name:    Name for the output files (e.g. "Daredevil_Devil2099")

        Returns:
            PackResult with pak/utoc/ucas paths on success.
        """
        out_base = self.output_dir / f"{mod_name}_9999999_P"
        utoc_path = Path(f"{out_base}.utoc")
        ucas_path = Path(f"{out_base}.ucas")
        pak_path = Path(f"{out_base}.pak")

        # Clean previous output
        for p in [utoc_path, ucas_path, pak_path]:
            if p.exists():
                p.unlink()

        cmd = [
            str(RETOC_EXE),
            "to-zen",
            str(staging_dir),
            str(utoc_path),
            "--version", "UE5_3",
            "-v",
        ]

        try:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=300,
            )
        except FileNotFoundError:
            return PackResult(False, error=f"retoc.exe not found at {RETOC_EXE}")
        except subprocess.TimeoutExpired:
            return PackResult(False, error="retoc to-zen timed out (300s)")

        output = (result.stdout + "\n" + result.stderr).strip()

        if result.returncode != 0:
            return PackResult(
                False,
                error=f"retoc to-zen failed (rc={result.returncode}):\n{output}",
            )

        if not utoc_path.exists() or not ucas_path.exists():
            return PackResult(
                False,
                error=f"retoc to-zen ran but output not found:\n{output}",
            )

        return PackResult(
            True,
            pak_path=pak_path if pak_path.exists() else None,
            utoc_path=utoc_path,
            ucas_path=ucas_path,
        )

    # ------------------------------------------------------------------
    # Deploy to ~mods
    # ------------------------------------------------------------------

    def deploy_to_mods(self, pack_result: PackResult) -> PackResult:
        """
        Copy .pak, .utoc, .ucas into the game's ~mods folder.
        Creates ~mods if it doesn't exist.  After a successful copy the
        source files in the output directory are deleted to avoid clutter.
        """
        if not self.game_paks_dir:
            return PackResult(False, error="Game Paks directory not configured")

        mods_dir = self.game_paks_dir / "~mods"
        mods_dir.mkdir(parents=True, exist_ok=True)

        files_to_copy = [
            pack_result.pak_path,
            pack_result.utoc_path,
            pack_result.ucas_path,
        ]

        for src in files_to_copy:
            if src and src.exists():
                dst = mods_dir / src.name
                shutil.copy2(str(src), str(dst))

        # Clean up the source output files now that they've been deployed
        for src in files_to_copy:
            if src and src.exists():
                try:
                    src.unlink()
                except OSError:
                    pass

        return PackResult(
            True,
            pak_path=mods_dir / pack_result.pak_path.name if pack_result.pak_path else None,
            utoc_path=mods_dir / pack_result.utoc_path.name if pack_result.utoc_path else None,
            ucas_path=mods_dir / pack_result.ucas_path.name if pack_result.ucas_path else None,
        )
