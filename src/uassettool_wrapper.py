"""
Wrapper around XzantGaming/UAssetToolRivals for IoStore mod creation.

Tools:
    tools/uassettool/UAssetTool.exe     — UAsset CLI (create_mod_iostore, etc.)
    tools/uassettool/*.usmap            — Unversioned property mappings

This replaces retoc to-zen for packing because UAssetTool applies
Marvel Rivals–specific auto-fixes during conversion:
    • SkeletalMesh FGameplayTagContainer padding
    • MaterialTag injection from MaterialTagAssetUserData
    • Companion PAK with chunk names

retoc is still used for IoStore → legacy extraction (to-legacy).
"""

import subprocess
from pathlib import Path
from dataclasses import dataclass
from typing import Optional


PROJECT_ROOT = Path(__file__).resolve().parent.parent
UASSETTOOL_EXE = PROJECT_ROOT / "tools" / "uassettool" / "UAssetTool.exe"
UASSETTOOL_DIR = PROJECT_ROOT / "tools" / "uassettool"


def _find_usmap() -> Optional[Path]:
    """Find the first .usmap file in the uassettool directory."""
    for p in UASSETTOOL_DIR.iterdir():
        if p.suffix == ".usmap":
            return p
    return None


@dataclass
class PackResult:
    success: bool
    pak_path: Optional[Path] = None
    utoc_path: Optional[Path] = None
    ucas_path: Optional[Path] = None
    error: str = ""


class UAssetToolWrapper:
    """
    Drives UAssetTool to pack mods into IoStore format with
    Marvel Rivals auto-fixes.

    Expects UAssetTool.exe at: tools/uassettool/UAssetTool.exe
    """

    def __init__(self, output_dir: Path | str) -> None:
        self.output_dir = Path(output_dir)
        self.output_dir.mkdir(parents=True, exist_ok=True)
        self.usmap_path = _find_usmap()

    # ------------------------------------------------------------------
    # Validation
    # ------------------------------------------------------------------

    def validate(self) -> list[str]:
        """Return a list of problems (empty = all good)."""
        problems: list[str] = []
        if not UASSETTOOL_EXE.is_file():
            problems.append(f"UAssetTool.exe not found at: {UASSETTOOL_EXE}")
        if not self.usmap_path or not self.usmap_path.is_file():
            problems.append(
                f"No .usmap file found in {UASSETTOOL_DIR}. "
                "Place a Marvel Rivals .usmap file there."
            )
        return problems

    # ------------------------------------------------------------------
    # Pack to IoStore
    # ------------------------------------------------------------------

    def pack_to_iostore(
        self,
        staging_dir: Path,
        mod_name: str,
    ) -> PackResult:
        """
        Pack a staging directory into IoStore format using UAssetTool
        create_mod_iostore.

        The staging_dir should contain the Marvel/Content/... tree with
        the renamed skin files (.uasset / .uexp / .ubulk).

        UAssetTool will:
          • Convert legacy assets to Zen/IoStore format
          • Inject MaterialTags for SkeletalMesh assets
          • Add FGameplayTagContainer padding
          • Create companion PAK with chunk names
          • Apply Oodle compression

        Args:
            staging_dir: Folder containing Marvel/Content/... asset tree
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

        # Build the input path — UAssetTool accepts a directory and
        # recursively finds all .uasset files inside it
        input_dir = staging_dir / "Marvel" / "Content"
        if not input_dir.is_dir():
            # Fall back to staging_dir itself
            input_dir = staging_dir

        cmd = [
            str(UASSETTOOL_EXE),
            "create_mod_iostore",
            str(out_base),
            str(input_dir),
        ]

        try:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=300,
            )
        except FileNotFoundError:
            return PackResult(
                False,
                error=f"UAssetTool.exe not found at {UASSETTOOL_EXE}",
            )
        except subprocess.TimeoutExpired:
            return PackResult(False, error="UAssetTool create_mod_iostore timed out (300s)")

        output = (result.stdout + "\n" + result.stderr).strip()

        # UAssetTool often exits with code 1 due to stderr warnings
        # even on success — check for output files instead
        if not utoc_path.exists() or not ucas_path.exists():
            return PackResult(
                False,
                error=f"UAssetTool create_mod_iostore failed — output not found:\n{output}",
            )

        return PackResult(
            True,
            pak_path=pak_path if pak_path.exists() else None,
            utoc_path=utoc_path,
            ucas_path=ucas_path,
        )
