"""
Skin Swap Engine for MR-SkinChanger.

Pipeline:
    1. Extract source skin files           (retoc to-legacy)
       — Meshes, Materials, Textures (supports both mesh-swap and
         retexture-only skins)
    2. Extract default skin files          (retoc to-legacy) for physics assets
    3. Extract weapon files for both       (retoc to-legacy)
       — Meshes, Materials, Textures within each weapon slot
    4. Extract VFX files for both          (retoc to-legacy) for recolors
    5. Stage mod directory:
         a. Copy source files (Meshes/Materials/Textures), rename filenames
            (source_id -> target_id)
         b. Patch name map in .uasset files  (source_id -> target_id)
            — rewrites only the string table so UAssetTool registers
              the asset at the default skin's IoStore path
         c. Include default PhysicsAsset files
         d. Copy weapon files for matching weapon slots (Meshes/Materials/Textures)
         e. Copy VFX files for overlapping ability folders (recolors)
    6. Pack into IoStore format             (UAssetTool create_mod_iostore)
    7. Clean up temp directories
"""

import os
import re
import shutil
from pathlib import Path
from dataclasses import dataclass
from typing import Optional, Callable

from .retoc_wrapper import RetocWrapper
from .uassettool_wrapper import UAssetToolWrapper, PackResult
from .skin_database import CharacterInfo, SkinInfo
from .uasset_patcher import patch_skin_id_in_uasset


# Keywords that identify PhysicsAsset files from the default skin.
# These must always be included so the mesh has proper physics.
# Skeleton files are NOT included — the mesh references its own skeleton
# internally and the game resolves it from the base paks.
DEFAULT_PHYSICS_KEYWORDS = ("_PhysicsAsset", "SK_Physics_Death")


@dataclass
class SwapResult:
    """Outcome of a full skin swap operation."""
    success: bool
    pack_result: Optional[PackResult] = None
    mod_name: str = ""
    files_created: int = 0
    error: str = ""


LogCallback = Callable[[str], None]


def _noop_log(msg: str) -> None:
    """Default no-op logger."""


class SwapEngine:
    """
    Drives the complete skin-swap workflow:
        extract -> stage -> patch -> pack
    """

    def __init__(
        self,
        retoc: RetocWrapper,
        uassettool: UAssetToolWrapper,
    ) -> None:
        self.retoc = retoc
        self.uassettool = uassettool

    def create_skin_swap(
        self,
        character: CharacterInfo,
        source_skin: SkinInfo,
        log_callback: Optional[LogCallback] = None,
    ) -> SwapResult:
        """
        Build a complete mod that swaps the default skin with source_skin.
        """
        log = log_callback or _noop_log

        source_id = source_skin.skin_id
        target_id = character.default_skin_id
        char_id = character.char_id
        # Build a filesystem-safe mod name: replace spaces and strip any
        # characters that are illegal in Windows/Linux filenames.
        _raw_name = f"{character.name}_{source_skin.skin_name}".replace(" ", "_")
        mod_name = re.sub(r'[<>:"/\\|?*]', "", _raw_name)

        if source_id == target_id:
            return SwapResult(
                False,
                error="Source skin is the same as the default. Nothing to swap.",
            )

        # -- 1. Extract source skin ----------------------------------------
        log(f"Extracting source skin {source_id}...")
        src_extract = self.retoc.extract_skin(char_id, source_id)
        if not src_extract.success:
            return SwapResult(
                False,
                error=f"Source extraction failed: {src_extract.error}",
            )
        log(f"  {src_extract.files_extracted} files extracted")

        # -- 2. Extract default skin (for rig / physics) --------------------
        log(f"Extracting default skin {target_id} (rig + physics)...")
        def_extract = self.retoc.extract_skin(char_id, target_id)
        if not def_extract.success:
            self._cleanup(src_extract.output_dir)
            return SwapResult(
                False,
                error=f"Default extraction failed: {def_extract.error}",
            )
        log(f"  {def_extract.files_extracted} files extracted")

        # -- 3. Extract weapon files for both skins -----------------------
        log(f"Extracting source weapon files {source_id}...")
        src_wpn_extract = self.retoc.extract_skin_weapons(char_id, source_id)
        if src_wpn_extract.success:
            log(f"  {src_wpn_extract.files_extracted} weapon files extracted")
        else:
            log("  No source weapon files found (may not exist for this character)")
            src_wpn_extract = None

        log(f"Extracting default weapon files {target_id}...")
        def_wpn_extract = self.retoc.extract_skin_weapons(char_id, target_id)
        if def_wpn_extract.success:
            log(f"  {def_wpn_extract.files_extracted} weapon files extracted")
        else:
            log("  No default weapon files found (may not exist for this character)")
            def_wpn_extract = None

        # -- 4. Extract VFX files for both skins (recolor support) ---------
        log(f"Extracting source VFX files {source_id}...")
        src_vfx_extract = self.retoc.extract_skin_vfx(char_id, source_id)
        if src_vfx_extract.success:
            log(f"  {src_vfx_extract.files_extracted} VFX files extracted")
        else:
            log("  No source VFX files found")
            src_vfx_extract = None

        log(f"Extracting default VFX files {target_id}...")
        def_vfx_extract = self.retoc.extract_skin_vfx(char_id, target_id)
        if def_vfx_extract.success:
            log(f"  {def_vfx_extract.files_extracted} VFX files extracted")
        else:
            log("  No default VFX files found")
            def_vfx_extract = None

        # -- 5. Stage the mod directory ------------------------------------
        log("Staging mod files...")
        try:
            staging_dir, file_count = self._stage_mod(
                source_dir=src_extract.output_dir,
                default_dir=def_extract.output_dir,
                char_id=char_id,
                source_id=source_id,
                target_id=target_id,
                source_weapon_dir=(
                    src_wpn_extract.output_dir if src_wpn_extract else None
                ),
                default_weapon_dir=(
                    def_wpn_extract.output_dir if def_wpn_extract else None
                ),
                source_vfx_dir=(
                    src_vfx_extract.output_dir if src_vfx_extract else None
                ),
                default_vfx_dir=(
                    def_vfx_extract.output_dir if def_vfx_extract else None
                ),
                log=log,
            )
        except Exception as exc:
            self._cleanup(
                src_extract.output_dir,
                def_extract.output_dir,
                src_wpn_extract.output_dir if src_wpn_extract else None,
                def_wpn_extract.output_dir if def_wpn_extract else None,
                src_vfx_extract.output_dir if src_vfx_extract else None,
                def_vfx_extract.output_dir if def_vfx_extract else None,
            )
            return SwapResult(False, error=f"Staging failed: {exc}")

        log(f"  {file_count} files staged")

        # -- 6. Pack with UAssetTool ----------------------------------------
        log("Packing IoStore mod (UAssetTool)...")
        pack_res = self.uassettool.pack_to_iostore(staging_dir, mod_name)

        # Clean up temp directories
        self._cleanup(
            src_extract.output_dir,
            def_extract.output_dir,
            src_wpn_extract.output_dir if src_wpn_extract else None,
            def_wpn_extract.output_dir if def_wpn_extract else None,
            src_vfx_extract.output_dir if src_vfx_extract else None,
            def_vfx_extract.output_dir if def_vfx_extract else None,
            staging_dir,
        )

        if not pack_res.success:
            return SwapResult(False, error=f"Packing failed: {pack_res.error}")

        log("  IoStore files created")
        return SwapResult(
            success=True,
            pack_result=pack_res,
            mod_name=mod_name,
            files_created=file_count,
        )

    def _stage_mod(
        self,
        source_dir: Path,
        default_dir: Path,
        char_id: str,
        source_id: str,
        target_id: str,
        source_weapon_dir: Optional[Path] = None,
        default_weapon_dir: Optional[Path] = None,
        source_vfx_dir: Optional[Path] = None,
        default_vfx_dir: Optional[Path] = None,
        log: LogCallback = _noop_log,
    ) -> tuple[Path, int]:
        """
        Build the mod staging tree. Returns (staging_dir, total_file_count).
        """
        staging_dir = self.retoc.output_dir / "staging"
        if staging_dir.exists():
            shutil.rmtree(staging_dir)

        # Target directory mirrors the game content path.
        # The game's internal path is Marvel/Characters/<id>/<skin>/Meshes
        # and UAssetTool strips "Marvel/Content/" prefix (--game-path default),
        # so we need: staging/Marvel/Content/Marvel/Characters/...
        content_base = (
            staging_dir / "Marvel" / "Content" / "Marvel" / "Characters"
            / char_id / target_id
        )

        file_count = 0

        # -- Part 1: Source skin files (renamed, name-map patched) ---------
        # Source extraction may contain Meshes, Materials, and/or Textures.
        # For mesh-swap skins all three exist; for retexture-only skins
        # only Materials and Textures are present.  We preserve the
        # subfolder structure (Meshes/, Materials/, Textures/).
        source_files = self._find_asset_files(source_dir)
        log(f"  Source files found: {len(source_files)}")

        # Find the skin-level directory in the extraction tree so we can
        # derive relative paths (Meshes/foo.uasset, Materials/bar.uasset).
        source_skin_root = self._find_skin_root(source_dir, source_id)

        # Detect retexture-only skins: no Meshes directory in source.
        # Retexture materials are MaterialInstances that override
        # textures from the default skin's parent material.  We stage
        # them normally — the name-map patcher only rewrites self-
        # reference paths (FolderName + own object name) so the
        # parent-material import (which already uses the default skin
        # ID) is never touched and no circular reference is created.
        has_meshes = any(
            "\\Meshes\\" in str(f) or "/Meshes/" in str(f)
            for f in source_files
        )
        is_retexture = not has_meshes

        # Check if the source extraction contains any texture files.
        # Many skins (especially mesh-swap skins) have Materials but
        # NOT a Textures folder — the textures live elsewhere in the
        # game paks.  When textures are NOT staged we must tell the
        # patcher to leave T_* / /Textures/ references alone so
        # materials keep pointing at the source skin's original textures.
        has_textures = any(
            "\\Textures\\" in str(f) or "/Textures/" in str(f)
            for f in source_files
        )
        skip_texture_refs = not has_textures

        if is_retexture:
            log("  Retexture-only skin detected (no Meshes folder)")
        if skip_texture_refs:
            log("  No Textures folder — texture references will be preserved")

        for src_path in source_files:
            fname = src_path.name

            # Skip physics files from source -- we use default physics
            if any(kw in fname for kw in DEFAULT_PHYSICS_KEYWORDS):
                log(f"  Skipping source physics file: {fname}")
                continue

            # Determine the subfolder (Meshes, Materials, Textures, etc.)
            if source_skin_root:
                try:
                    rel = src_path.relative_to(source_skin_root)
                except ValueError:
                    # File is not under the skin root (e.g. a VFX file
                    # captured by a broad extraction filter).  Skip it —
                    # VFX files are handled separately in _stage_vfx().
                    continue
            else:
                rel = Path(fname)

            # Rename: replace source_id with target_id in filename
            new_name = fname.replace(source_id, target_id)
            rel_renamed = rel.parent / new_name

            dst_path = content_base / rel_renamed
            dst_path.parent.mkdir(parents=True, exist_ok=True)

            shutil.copy2(str(src_path), str(dst_path))
            if new_name != fname:
                log(f"  Renamed: {fname} -> {new_name}")

            # Patch the name map inside .uasset files so that the
            # internal FolderName / object names reference the target
            # skin path instead of the source skin path.  Without
            # this, UAssetTool registers the asset at the source path
            # in IoStore and the game never loads it as a replacement.
            if dst_path.suffix == ".uasset":
                try:
                    modified = patch_skin_id_in_uasset(
                        dst_path, source_id, target_id,
                        skip_texture_refs=skip_texture_refs,
                    )
                    if modified:
                        log(f"  Patched name map: {len(modified)} entries")
                except Exception as exc:
                    log(f"  WARNING: Name map patch failed for {new_name}: {exc}")

            file_count += 1

        # -- Part 2: Default PhysicsAsset files -----------------------------
        default_files = self._find_asset_files(default_dir)
        physics_files = [
            f for f in default_files
            if any(kw in f.name for kw in DEFAULT_PHYSICS_KEYWORDS)
        ]
        log(f"  Default physics files to include: {len(physics_files)}")

        # Physics assets live under Meshes/
        mesh_dir = content_base / "Meshes"
        mesh_dir.mkdir(parents=True, exist_ok=True)

        for phys_path in physics_files:
            dst_path = mesh_dir / phys_path.name
            shutil.copy2(str(phys_path), str(dst_path))
            log(f"  Physics file: {phys_path.name}")
            file_count += 1

        # -- Part 3: Weapon files (matching slots only) ---------------------
        if source_weapon_dir and default_weapon_dir:
            wpn_count = self._stage_weapons(
                source_weapon_dir=source_weapon_dir,
                default_weapon_dir=default_weapon_dir,
                content_base=content_base,
                char_id=char_id,
                source_id=source_id,
                target_id=target_id,
                log=log,
            )
            file_count += wpn_count
        elif source_weapon_dir:
            log("  Skipping weapon staging: no default weapon files extracted")
        else:
            log("  No weapon files to stage")

        # -- Part 4: VFX recolor files (overlapping ability folders) -------
        if source_vfx_dir and default_vfx_dir:
            vfx_count = self._stage_vfx(
                source_vfx_dir=source_vfx_dir,
                default_vfx_dir=default_vfx_dir,
                staging_dir=staging_dir,
                char_id=char_id,
                source_id=source_id,
                target_id=target_id,
                log=log,
            )
            file_count += vfx_count
        elif source_vfx_dir:
            log("  Skipping VFX staging: no default VFX files extracted")
        else:
            log("  No VFX files to stage")

        return staging_dir, file_count

    # ------------------------------------------------------------------
    # Weapon staging
    # ------------------------------------------------------------------

    def _stage_weapons(
        self,
        source_weapon_dir: Path,
        default_weapon_dir: Path,
        content_base: Path,
        char_id: str,
        source_id: str,
        target_id: str,
        log: LogCallback = _noop_log,
    ) -> int:
        """
        Stage weapon files for matching weapon slots.

        Weapons live under ``Characters/{char_id}/{skin_id}/Weapons/{slot}/``.
        Each slot can contain sub-folders like ``Meshes/``, ``Materials/``,
        ``Texture/``, etc.  Only weapon slots that exist in BOTH the source
        and default skins are staged.

        All sub-folders (including Materials) are staged.  The uasset
        patcher is context-aware and will correctly handle material
        files without creating circular references.

        For each common slot we copy all asset sub-folders from the source
        skin to the default skin's path so the game loads them.

        Returns the number of files staged.
        """
        source_slots = self._find_weapon_asset_dirs(source_weapon_dir)
        default_slots = self._find_weapon_asset_dirs(default_weapon_dir)

        if not source_slots:
            log("  No source weapon directories found")
            return 0

        # Only process weapon slots present in both source and default
        common_slots = sorted(set(source_slots.keys()) & set(default_slots.keys()))
        source_only = sorted(set(source_slots.keys()) - set(default_slots.keys()))
        default_only = sorted(set(default_slots.keys()) - set(source_slots.keys()))

        if common_slots:
            log(f"  Weapon slots to swap: {', '.join(common_slots)}")
        if source_only:
            log(f"  Source-only weapon slots (skipped): {', '.join(source_only)}")
        if default_only:
            log(f"  Default-only weapon slots (not swapped): {', '.join(default_only)}")

        file_count = 0

        for slot_name in common_slots:
            src_asset_dirs = source_slots[slot_name]
            def_asset_dirs = default_slots[slot_name]

            default_weapons_root = self._find_weapons_root(default_weapon_dir)
            if default_weapons_root is None:
                log("  WARNING: Cannot find Weapons root in default extraction")
                continue

            # Stage each asset sub-folder (Meshes, Materials, Texture, etc.)
            for sub_name, src_sub_dir in src_asset_dirs.items():

                # Use the default's corresponding sub-folder to derive
                # the correct staging path.  If the default doesn't have
                # this exact sub-folder, derive it from the source structure.
                if sub_name in def_asset_dirs:
                    def_sub_dir = def_asset_dirs[sub_name]
                    rel_path = def_sub_dir.relative_to(default_weapons_root)
                else:
                    # Source has a sub-folder that default doesn't — still
                    # stage it at the expected path under the default slot.
                    # Find the default slot root from any of its sub-dirs.
                    any_def_sub = next(iter(def_asset_dirs.values()))
                    slot_root = any_def_sub.parent
                    rel_path = slot_root.relative_to(default_weapons_root) / sub_name

                target_wpn_dir = content_base / "Weapons" / rel_path
                target_wpn_dir.mkdir(parents=True, exist_ok=True)

                # Copy and process each file in the source sub-dir
                src_files = sorted(src_sub_dir.glob("*"))
                for src_path in src_files:
                    if not src_path.is_file():
                        continue
                    if src_path.suffix not in (".uasset", ".uexp", ".ubulk"):
                        continue

                    fname = src_path.name

                    # Skip physics files from source — use default physics
                    if any(kw in fname for kw in DEFAULT_PHYSICS_KEYWORDS):
                        continue

                    # Rename source_id → target_id in filename
                    new_name = fname.replace(source_id, target_id)
                    dst_path = target_wpn_dir / new_name
                    shutil.copy2(str(src_path), str(dst_path))

                    # Patch .uasset name map / FName Numbers
                    if dst_path.suffix == ".uasset":
                        try:
                            modified = patch_skin_id_in_uasset(
                                dst_path, source_id, target_id
                            )
                            if modified:
                                log(f"  [{slot_name}/{sub_name}] Patched: {new_name} ({len(modified)} entries)")
                        except Exception as exc:
                            log(f"  [{slot_name}/{sub_name}] WARNING: Patch failed for {new_name}: {exc}")

                    file_count += 1

            # Include default physics files for this weapon slot (Meshes only)
            if "Meshes" in def_asset_dirs:
                default_mesh_dir = def_asset_dirs["Meshes"]
                default_mesh_files = sorted(default_mesh_dir.glob("*"))

                def_meshes_rel = default_mesh_dir.relative_to(default_weapons_root)
                physics_target = content_base / "Weapons" / def_meshes_rel
                physics_target.mkdir(parents=True, exist_ok=True)

                for phys_path in default_mesh_files:
                    if not phys_path.is_file():
                        continue
                    if not any(kw in phys_path.name for kw in DEFAULT_PHYSICS_KEYWORDS):
                        continue
                    if phys_path.suffix not in (".uasset", ".uexp"):
                        continue

                    dst_path = physics_target / phys_path.name
                    if not dst_path.exists():
                        shutil.copy2(str(phys_path), str(dst_path))
                        log(f"  [{slot_name}] Physics: {phys_path.name}")
                        file_count += 1

        return file_count

    @staticmethod
    def _find_weapon_asset_dirs(weapon_extract_dir: Path) -> dict[str, dict[str, Path]]:
        """
        Discover weapon asset directories under an extraction root.

        Walks the directory tree looking for ``Weapons/{slot_name}/{sub}/``
        patterns, where *sub* is any asset sub-folder such as ``Meshes``,
        ``Materials``, or ``Texture``.

        Returns a nested dict::

            { slot_name: { sub_folder_name: Path } }

        For example::

            {
                "DevouringSymbiote": {
                    "Meshes": Path(...),
                    "Materials": Path(...),
                    "Texture": Path(...),
                },
            }

        The slot name is the directory name directly under ``Weapons/``.
        """
        slots: dict[str, dict[str, Path]] = {}
        weapons_root = SwapEngine._find_weapons_root(weapon_extract_dir)
        if weapons_root is None:
            return slots

        # Walk directly under weapons_root to find slot-level dirs,
        # then list their immediate sub-directories as asset types.
        for slot_dir in sorted(weapons_root.iterdir()):
            if not slot_dir.is_dir():
                continue
            slot_name = slot_dir.name
            asset_subs: dict[str, Path] = {}
            for sub_dir in sorted(slot_dir.iterdir()):
                if sub_dir.is_dir():
                    # Check that it actually contains asset files
                    has_assets = any(
                        f.suffix in (".uasset", ".uexp", ".ubulk")
                        for f in sub_dir.rglob("*")
                        if f.is_file()
                    )
                    if has_assets:
                        asset_subs[sub_dir.name] = sub_dir
            if asset_subs:
                slots[slot_name] = asset_subs

        return slots

    @staticmethod
    def _find_weapons_root(extract_dir: Path) -> Optional[Path]:
        """
        Find the ``Weapons`` directory within an extraction tree.

        The extraction places files under a deep ``Marvel/Content/...``
        hierarchy, so we search for the first directory named ``Weapons``.
        """
        for dirpath, dirnames, _ in os.walk(extract_dir):
            if "Weapons" in dirnames:
                return Path(dirpath) / "Weapons"
        return None

    # ------------------------------------------------------------------
    # VFX recolor staging
    # ------------------------------------------------------------------

    # VFX sub-directory types under ``Marvel/VFX/``
    _VFX_TYPES = ("Meshes", "Particles", "Textures", "Materials",
                  "DecalAnimation", "VAT")

    def _stage_vfx(
        self,
        source_vfx_dir: Path,
        default_vfx_dir: Path,
        staging_dir: Path,
        char_id: str,
        source_id: str,
        target_id: str,
        log: LogCallback = _noop_log,
    ) -> int:
        """
        Stage VFX recolor files for overlapping ability subfolders.

        VFX assets are extracted to a tree like::

            {extract_dir}/Marvel/Content/Marvel/VFX/{Type}/Characters/{charID}/{skinID}/{abilityFolder}/...

        For each VFX type (Particles, Materials, Textures, Meshes, etc.)
        we find ability subfolders (e.g. ``105531``) that exist in BOTH
        the source and default skins. Only those overlapping subfolders
        are staged — files from the source skin are placed at the
        default skin's path so the game loads the recolored VFX.

        Returns the number of files staged.
        """
        file_count = 0

        for vfx_type in self._VFX_TYPES:
            # Find the skin-ID-level directories for this VFX type
            src_skin_dir = self._find_vfx_skin_dir(
                source_vfx_dir, vfx_type, char_id, source_id
            )
            def_skin_dir = self._find_vfx_skin_dir(
                default_vfx_dir, vfx_type, char_id, target_id
            )

            if src_skin_dir is None or def_skin_dir is None:
                continue

            # Derive the correct staging path from the default's actual
            # extracted directory structure.  The extraction places files
            # under  {extract_dir}/Marvel/Content/Marvel/VFX/...  and we
            # need to reproduce the same tree under staging/.
            # Find "Marvel/Content" in the default's path to get the
            # relative game path.
            target_vfx_base = self._vfx_staging_path(
                def_skin_dir, default_vfx_dir, staging_dir
            )
            if target_vfx_base is None:
                log(f"  [VFX/{vfx_type}] WARNING: Cannot compute staging path")
                continue

            # NOTE: We intentionally skip loose files (files directly in
            # the skin directory, e.g. VFX Bundle meshes, material instances,
            # textures).  Loose VFX files have skin-specific filenames
            # (MI_1055500_*, SM_1055500_*, T_1055500_*) that are different
            # from what the default skin's particles reference.  Renaming
            # them to the target ID just creates orphan files that nothing
            # loads.  Only ability-subfolder files (e.g. NS_105531_*)
            # share ability-based names across skins and are actually
            # picked up by the default skin's ChildBP.

            # -- Ability subfolders (e.g. 105531) --------------------------
            # Discover ability subfolders in each
            src_ability_dirs = self._list_ability_dirs(src_skin_dir)
            def_ability_dirs = self._list_ability_dirs(def_skin_dir)

            if not src_ability_dirs or not def_ability_dirs:
                continue

            common = sorted(set(src_ability_dirs.keys()) & set(def_ability_dirs.keys()))
            source_only = sorted(set(src_ability_dirs.keys()) - set(def_ability_dirs.keys()))

            if not common:
                continue

            log(f"  VFX/{vfx_type}: overlapping ability folders: {', '.join(common)}")
            if source_only:
                log(f"  VFX/{vfx_type}: source-only folders (skipped): {', '.join(source_only)}")

            for ability_name in common:
                src_ab_dir = src_ability_dirs[ability_name]
                def_ab_dir = def_ability_dirs[ability_name]

                # Walk all files in the source ability folder (recursively)
                for src_path in sorted(src_ab_dir.rglob("*")):
                    if not src_path.is_file():
                        continue
                    if src_path.suffix not in (".uasset", ".uexp", ".ubulk"):
                        continue

                    # Compute the relative path from the source skin dir
                    # e.g. 105531/NS_105531_Release_01.uasset
                    rel_from_skin = src_path.relative_to(src_skin_dir)

                    # Build the target path using the already-computed base
                    # Replace source_id with target_id in the relative path parts
                    rel_str = str(rel_from_skin).replace(source_id, target_id)
                    dst_path = target_vfx_base / rel_str
                    dst_path.parent.mkdir(parents=True, exist_ok=True)

                    # Rename source_id → target_id in filename
                    new_name = dst_path.name.replace(source_id, target_id)
                    if new_name != dst_path.name:
                        dst_path = dst_path.parent / new_name

                    shutil.copy2(str(src_path), str(dst_path))

                    # Patch .uasset name map so IoStore paths point to
                    # the default skin directory
                    if dst_path.suffix == ".uasset":
                        try:
                            modified = patch_skin_id_in_uasset(
                                dst_path, source_id, target_id
                            )
                            if modified:
                                log(f"  [VFX/{vfx_type}/{ability_name}] Patched: "
                                    f"{dst_path.name} ({len(modified)} entries)")
                        except Exception as exc:
                            log(f"  [VFX/{vfx_type}/{ability_name}] WARNING: "
                                f"Patch failed for {dst_path.name}: {exc}")

                    file_count += 1

        # -- NPCI: Niagara Parameter Collection Instance -------------------
        # NPCI files define per-skin colour overrides for VFX (BaseColor,
        # SpecColor, etc.).  Located at:
        #   VFX/Particles/NiagaraParameterCollection/{charID}/NPCI_{skinID}
        # We rename the source's NPCI to the target's to override the
        # default VFX colours.  The name map does NOT contain the skin ID
        # so no binary patching is needed — only a filename rename.
        npci_count = self._stage_npci(
            source_vfx_dir=source_vfx_dir,
            default_vfx_dir=default_vfx_dir,
            staging_dir=staging_dir,
            char_id=char_id,
            source_id=source_id,
            target_id=target_id,
            log=log,
        )
        file_count += npci_count

        if file_count:
            log(f"  VFX recolor: {file_count} files staged total")
        else:
            log("  No overlapping VFX ability folders found (no recolor files)")

        return file_count

    @staticmethod
    def _find_vfx_skin_dir(
        extract_dir: Path,
        vfx_type: str,
        char_id: str,
        skin_id: str,
    ) -> Optional[Path]:
        """
        Find the skin-level directory for a VFX type in an extraction tree.

        Looks for a directory named *skin_id* that sits somewhere below
        ``VFX/{vfx_type}/Characters/{char_id}/`` in the extraction output.

        Some VFX types use a flat layout::

            VFX/Particles/Characters/1055/1055500/

        while others have an extra nesting level::

            VFX/Materials/Characters/1055/Materials/1055500/

        This method handles both cases by walking the tree and checking
        that *char_id* appears as an ancestor of the *skin_id* directory.
        """
        for dirpath, dirnames, _ in os.walk(extract_dir):
            p = Path(dirpath)
            if p.name != skin_id:
                continue
            # Verify the path contains the expected VFX type and Characters
            # hierarchy with the char_id somewhere above.
            path_str = str(p).replace("\\", "/")
            if (f"VFX/{vfx_type}/" in path_str
                    and f"/Characters/{char_id}/" in path_str):
                return p
        return None

    @staticmethod
    def _vfx_staging_path(
        def_skin_dir: Path,
        extract_root: Path,
        staging_dir: Path,
    ) -> Optional[Path]:
        """
        Compute the staging directory for VFX files based on the default
        skin's actual extracted path.

        The extraction places files under::

            {extract_root}/Marvel/Content/Marvel/VFX/{Type}/.../{skinID}/

        We reproduce the same ``Marvel/Content/...`` tree under *staging_dir*
        so UAssetTool registers assets at the correct IoStore path.

        Returns the staging path corresponding to the default skin's dir,
        or None if the path structure can't be parsed.
        """
        try:
            rel = def_skin_dir.relative_to(extract_root)
        except ValueError:
            return None
        return staging_dir / rel

    @staticmethod
    def _list_ability_dirs(skin_dir: Path) -> dict[str, Path]:
        """
        List immediate subdirectories of a VFX skin directory.

        These are "ability folders" like ``105531`` that group VFX assets
        by ability. Some VFX skin dirs also contain loose files directly
        (e.g. SM_Bundle meshes); those are individual files and not
        ability folders, so we only return actual directories.

        Returns a dict mapping folder_name → Path.
        """
        result: dict[str, Path] = {}
        if not skin_dir.is_dir():
            return result
        for child in sorted(skin_dir.iterdir()):
            if child.is_dir():
                result[child.name] = child
        return result

    # ------------------------------------------------------------------
    # NPCI staging (Niagara Parameter Collection Instance)
    # ------------------------------------------------------------------

    @staticmethod
    def _stage_npci(
        source_vfx_dir: Path,
        default_vfx_dir: Path,
        staging_dir: Path,
        char_id: str,
        source_id: str,
        target_id: str,
        log: LogCallback = _noop_log,
    ) -> int:
        """
        Stage the source skin's NPCI file renamed to the target skin ID.

        NPCI (Niagara Parameter Collection Instance) files define per-skin
        colour overrides for VFX.  They are located at::

            VFX/Particles/NiagaraParameterCollection/{charID}/NPCI_{skinID}.uasset

        Each NPCI stores ``LinearColor`` values for parameters like
        ``NPC.Venom.BaseColor`` / ``BaseColor2`` / ``SpecColor``.  By
        staging the source skin's NPCI with the target skin's filename,
        all VFX that read from the NPC parameters will automatically
        use the source skin's colour scheme.

        The NPCI name map does NOT contain the skin ID (the internal
        object name is just ``NPCI``), so no binary patching is needed.
        Only the filename needs to be renamed.

        Returns the number of files staged (0, 2, or more).
        """
        file_count = 0

        # Locate the source NPCI files
        src_npci = SwapEngine._find_npci_file(
            source_vfx_dir, char_id, source_id
        )
        if src_npci is None:
            return 0

        # Compute the staging path.  We need to mirror the game path:
        #   Marvel/Content/Marvel/VFX/Particles/NiagaraParameterCollection/{charID}/
        # We derive it from the source file's actual extracted path.
        npc_dir = src_npci.parent  # .../NiagaraParameterCollection/{charID}/
        try:
            rel_dir = npc_dir.relative_to(source_vfx_dir)
        except ValueError:
            log("  [VFX/NPCI] WARNING: Cannot compute staging path")
            return 0

        target_dir = staging_dir / rel_dir
        target_dir.mkdir(parents=True, exist_ok=True)

        # Copy all file extensions (.uasset, .uexp, .ubulk)
        stem = f"NPCI_{source_id}"
        target_stem = f"NPCI_{target_id}"
        for ext in (".uasset", ".uexp", ".ubulk"):
            src_path = src_npci.parent / f"{stem}{ext}"
            if src_path.exists():
                dst_path = target_dir / f"{target_stem}{ext}"
                shutil.copy2(str(src_path), str(dst_path))
                file_count += 1

        if file_count:
            log(f"  VFX/NPCI: staged {src_npci.name} -> {target_stem} "
                f"({file_count} files)")

        return file_count

    @staticmethod
    def _find_npci_file(
        vfx_extract_dir: Path,
        char_id: str,
        skin_id: str,
    ) -> Optional[Path]:
        """
        Find the NPCI .uasset file for a skin in a VFX extraction tree.

        Searches for ``NPCI_{skin_id}.uasset`` under the
        ``NiagaraParameterCollection/{char_id}/`` directory.
        """
        target_name = f"NPCI_{skin_id}.uasset"
        for dirpath, _, filenames in os.walk(vfx_extract_dir):
            if target_name in filenames:
                candidate = Path(dirpath) / target_name
                path_str = str(candidate).replace("\\", "/")
                if f"NiagaraParameterCollection/{char_id}/" in path_str:
                    return candidate
        return None

    # ------------------------------------------------------------------
    # Generic helpers
    # ------------------------------------------------------------------

    @staticmethod
    def _find_skin_root(extract_dir: Path, skin_id: str) -> Optional[Path]:
        """
        Find the skin-level directory in a character extraction tree.

        The extraction places files under::

            {extract_dir}/Marvel/Content/Marvel/Characters/{charID}/{skinID}/

        This method walks the tree and returns the directory named
        *skin_id* that has ``/Characters/`` as an ancestor — i.e. the
        root from which Meshes/, Materials/, Textures/ branch off.
        """
        for dirpath, dirnames, _ in os.walk(extract_dir):
            if skin_id in dirnames:
                candidate = Path(dirpath) / skin_id
                path_str = str(candidate).replace("\\", "/")
                if "/Characters/" in path_str:
                    return candidate
        return None

    @staticmethod
    def _find_asset_files(base_dir: Path) -> list[Path]:
        """Walk base_dir and return all .uasset / .uexp / .ubulk files."""
        result: list[Path] = []
        for root, _dirs, files in os.walk(base_dir):
            for f in files:
                if f.endswith((".uasset", ".uexp", ".ubulk")):
                    result.append(Path(root) / f)
        return sorted(result)

    @staticmethod
    def _cleanup(*dirs: Optional[Path]) -> None:
        """Remove temp directories, ignoring errors."""
        for d in dirs:
            if d and d.exists():
                try:
                    shutil.rmtree(d)
                except OSError:
                    pass
