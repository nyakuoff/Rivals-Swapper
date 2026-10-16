"""
Skin Swap Engine for MR-SkinChanger — Project Galacta output.

Pipeline:
    1. Extract the source skin (retoc to-legacy)
    2. Copy its main skeletal mesh into
       /Game/Marvel/ProjectGalacta/Meshes/{HeroID}/{DefaultSkinID}/{ModFolder}/
       and relocate the package there (no vanilla paths are overridden)
    3. Write a GAL_ModSkinConfig DataAsset next to it
    4. Pack into IoStore format (UAssetTool)

In-game, Project Galacta's Skin Swapper applies the mesh once the Galacta
Companion App has rebuilt its manifest.
"""

import re
import shutil
from pathlib import Path
from dataclasses import dataclass
from typing import Optional, Callable

from .retoc_wrapper import RetocWrapper
from .uassettool_wrapper import UAssetToolWrapper, PackResult
from .skin_database import CharacterInfo, SkinInfo
from .uasset_patcher import relocate_package
from .galacta_config import galacta_mod_dir, write_skin_config

AUTHOR = "Rivals Swapper"


@dataclass
class SwapResult:
    success: bool
    pack_result: Optional[PackResult] = None
    mod_name: str = ""
    error: str = ""


LogCallback = Callable[[str], None]


def _noop_log(msg: str) -> None:
    pass


class SwapEngine:

    def __init__(self, retoc: RetocWrapper, uassettool: UAssetToolWrapper) -> None:
        self.retoc = retoc
        self.uassettool = uassettool

    def create_skin_swap(
        self,
        character: CharacterInfo,
        source_skin: SkinInfo,
        log_callback: Optional[LogCallback] = None,
    ) -> SwapResult:
        log = log_callback or _noop_log

        source_id = source_skin.skin_id
        target_id = character.default_skin_id
        char_id = character.char_id
        _raw_name = f"{character.name}_{source_skin.skin_name}".replace(" ", "_")
        mod_name = re.sub(r'[<>:"/\\|?*]', "", _raw_name)
        # UE package paths only accept a restricted character set.
        mod_folder = re.sub(r"\W", "", mod_name, flags=re.ASCII) or source_id

        if source_id == target_id:
            return SwapResult(False, error="Source skin is the same as the default. Nothing to swap.")

        log(f"Extracting source skin {source_id}...")
        src_extract = self.retoc.extract_skin(char_id, source_id)
        if not src_extract.success:
            return SwapResult(False, error=f"Source extraction failed: {src_extract.error}")
        log(f"  {src_extract.files_extracted} files extracted")

        mesh_name = f"SK_{char_id}_{source_id}"
        mesh = next(src_extract.output_dir.rglob(f"Meshes/{mesh_name}.uasset"), None)
        if mesh is None:
            self._cleanup(src_extract.output_dir)
            return SwapResult(
                False,
                error=f"{source_skin.skin_name} has no mesh of its own ({mesh_name} not found). "
                      "Project Galacta can only swap meshes, so retexture-only skins aren't supported.",
            )

        log("Staging Project Galacta mod...")
        staging_dir = self.retoc.output_dir / "staging"
        if staging_dir.exists():
            shutil.rmtree(staging_dir)
        content_dir = staging_dir / "Marvel" / "Content"
        mod_dir = galacta_mod_dir(char_id, target_id, mod_folder)
        dst_dir = content_dir / mod_dir.removeprefix("/Game/")
        dst_dir.mkdir(parents=True)

        mesh_package = f"{mod_dir}/{mesh_name}"
        try:
            for f in mesh.parent.glob(f"{mesh_name}.*"):
                shutil.copy2(f, dst_dir / f.name)
            old_path = relocate_package(dst_dir / mesh.name, mesh_package)
            log(f"  Mesh: {old_path} -> {mesh_package}")
            write_skin_config(
                content_dir, char_id, target_id, mod_folder,
                source_skin.skin_name, AUTHOR, mesh_package,
            )
            log(f"  Skin Config: {mod_dir}/GAL_SkinConfig")
        except (OSError, ValueError, RuntimeError) as exc:
            self._cleanup(src_extract.output_dir, staging_dir)
            return SwapResult(False, error=f"Staging failed: {exc}")

        log("Packing IoStore mod (UAssetTool)...")
        pack_res = self.uassettool.pack_to_iostore(staging_dir, mod_name)
        self._cleanup(src_extract.output_dir, staging_dir)

        if not pack_res.success:
            return SwapResult(False, error=f"Packing failed: {pack_res.error}")

        log("  IoStore files created")
        return SwapResult(success=True, pack_result=pack_res, mod_name=mod_name)

    @staticmethod
    def _cleanup(*dirs: Optional[Path]) -> None:
        for d in dirs:
            if d and d.exists():
                try:
                    shutil.rmtree(d)
                except OSError:
                    pass
