"""
Skin Swap Engine for MR-SkinChanger.

Pipeline:
    1. Extract source + default skin files (retoc to-legacy)
    2. Extract weapon files for both skins
    3. Stage mod: copy/rename files, patch name maps, include default physics
    4. Pack into IoStore format (UAssetTool)
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
from .uasset_patcher import patch_skin_id_in_uasset, patch_raw_bytes_in_file


DEFAULT_PHYSICS_KEYWORDS = ("_PhysicsAsset",)
SKIP_FILE_KEYWORDS = ("SK_Physics_Death", "Lobby_Physics.", "SK_Shell_Lobby", "SK_Lobby_Shell")


@dataclass
class SwapResult:
    success: bool
    pack_result: Optional[PackResult] = None
    mod_name: str = ""
    files_created: int = 0
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

        if source_id == target_id:
            return SwapResult(False, error="Source skin is the same as the default. Nothing to swap.")

        log(f"Extracting source skin {source_id}...")
        src_extract = self.retoc.extract_skin(char_id, source_id)
        if not src_extract.success:
            return SwapResult(False, error=f"Source extraction failed: {src_extract.error}")
        log(f"  {src_extract.files_extracted} files extracted")

        log(f"Extracting default skin {target_id} (rig + physics)...")
        def_extract = self.retoc.extract_skin(char_id, target_id)
        if not def_extract.success:
            self._cleanup(src_extract.output_dir)
            return SwapResult(False, error=f"Default extraction failed: {def_extract.error}")
        log(f"  {def_extract.files_extracted} files extracted")

        log(f"Extracting source weapon files {source_id}...")
        src_wpn_extract = self.retoc.extract_skin_weapons(char_id, source_id)
        if src_wpn_extract.success:
            log(f"  {src_wpn_extract.files_extracted} weapon files extracted")
        else:
            src_wpn_extract = None

        log(f"Extracting default weapon files {target_id}...")
        def_wpn_extract = self.retoc.extract_skin_weapons(char_id, target_id)
        if def_wpn_extract.success:
            log(f"  {def_wpn_extract.files_extracted} weapon files extracted")
        else:
            def_wpn_extract = None

        log("Staging mod files...")
        try:
            staging_dir, file_count = self._stage_mod(
                source_dir=src_extract.output_dir,
                default_dir=def_extract.output_dir,
                char_id=char_id,
                source_id=source_id,
                target_id=target_id,
                source_weapon_dir=(src_wpn_extract.output_dir if src_wpn_extract else None),
                default_weapon_dir=(def_wpn_extract.output_dir if def_wpn_extract else None),
                log=log,
            )
        except Exception as exc:
            self._cleanup(
                src_extract.output_dir, def_extract.output_dir,
                src_wpn_extract.output_dir if src_wpn_extract else None,
                def_wpn_extract.output_dir if def_wpn_extract else None,
            )
            return SwapResult(False, error=f"Staging failed: {exc}")

        log(f"  {file_count} files staged")

        log("Packing IoStore mod (UAssetTool)...")
        pack_res = self.uassettool.pack_to_iostore(staging_dir, mod_name)

        self._cleanup(
            src_extract.output_dir, def_extract.output_dir,
            src_wpn_extract.output_dir if src_wpn_extract else None,
            def_wpn_extract.output_dir if def_wpn_extract else None,
            staging_dir,
        )

        if not pack_res.success:
            return SwapResult(False, error=f"Packing failed: {pack_res.error}")

        log("  IoStore files created")
        return SwapResult(success=True, pack_result=pack_res, mod_name=mod_name, files_created=file_count)

    def _stage_mod(
        self,
        source_dir: Path,
        default_dir: Path,
        char_id: str,
        source_id: str,
        target_id: str,
        source_weapon_dir: Optional[Path] = None,
        default_weapon_dir: Optional[Path] = None,
        log: LogCallback = _noop_log,
    ) -> tuple[Path, int]:
        staging_dir = self.retoc.output_dir / "staging"
        if staging_dir.exists():
            shutil.rmtree(staging_dir)

        content_base = (
            staging_dir / "Marvel" / "Content" / "Marvel" / "Characters"
            / char_id / target_id
        )

        file_count = 0

        # -- Part 1: Source skin files -------------------------------------
        source_files = self._find_asset_files(source_dir)
        source_skin_root = self._find_skin_root(source_dir, source_id)

        has_meshes = any("\\Meshes\\" in str(f) or "/Meshes/" in str(f) for f in source_files)
        is_retexture = not has_meshes

        # Mesh-swap: stage Meshes/ only — the source skin's MI imports load
        # from base paks.  The mesh .uexp is raw-patched to fix
        # MaterialTagAssetUserData FString slot names so UAssetTool's tag
        # injection during create_mod_iostore resolves correctly.
        # Retexture: stage Textures/ only — the default mesh in base paks
        # already references T_{target_id}_* paths; staging renamed textures
        # is sufficient.  MI files are not staged to avoid overriding the
        # default mesh's material bindings.
        if is_retexture:
            log("  Retexture skin — staging Textures only")
        else:
            log("  Mesh-swap skin — staging Meshes only")

        for src_path in source_files:
            fname = src_path.name

            if any(kw in fname for kw in SKIP_FILE_KEYWORDS):
                continue
            if any(kw in fname for kw in DEFAULT_PHYSICS_KEYWORDS):
                continue

            src_str = str(src_path).replace("\\", "/")
            if not is_retexture and "/Meshes/" not in src_str:
                continue
            if is_retexture and "/Textures/" not in src_str and "/Texture/" not in src_str:
                continue

            if source_skin_root:
                try:
                    rel = src_path.relative_to(source_skin_root)
                except ValueError:
                    continue
            else:
                rel = Path(fname)

            new_name = fname.replace(source_id, target_id)
            dst_path = content_base / (rel.parent / new_name)
            dst_path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(str(src_path), str(dst_path))

            if new_name != fname:
                log(f"  Staged (renamed): {fname} -> {new_name}")
            else:
                log(f"  Staged: {fname}")

            if dst_path.suffix == ".uasset":
                dst_str = str(dst_path).replace("\\", "/")
                is_texture_file = "/Textures/" in dst_str or "/Texture/" in dst_str
                try:
                    modified = patch_skin_id_in_uasset(
                        dst_path, source_id, target_id,
                        skip_texture_refs=(not is_texture_file),
                        skip_material_refs=True,
                    )
                    if modified:
                        log(f"  Patched name map: {len(modified)} entries")
                except Exception as exc:
                    log(f"  WARNING: Name map patch failed for {new_name}: {exc}")

            elif dst_path.suffix == ".uexp" and not is_retexture:
                # Raw-patch mesh .uexp: fixes MaterialTagAssetUserData slot
                # FString entries so UAssetTool's tag injection during
                # create_mod_iostore resolves to the correct target slots.
                # ObjectReference bindings (MaterialInterface) are stored as
                # import-table indices, NOT inline strings, so they are
                # unaffected by this replace.
                try:
                    patch_raw_bytes_in_file(dst_path, source_id, target_id)
                except Exception as exc:
                    log(f"  WARNING: Mesh uexp patch failed for {new_name}: {exc}")

            file_count += 1

        # -- Part 2: Default PhysicsAsset files ----------------------------
        default_files = self._find_asset_files(default_dir)
        physics_files = [
            f for f in default_files
            if any(kw in f.name for kw in DEFAULT_PHYSICS_KEYWORDS)
            and not any(kw in f.name for kw in SKIP_FILE_KEYWORDS)
        ]

        mesh_dir = content_base / "Meshes"
        mesh_dir.mkdir(parents=True, exist_ok=True)
        for phys_path in physics_files:
            dst_path = mesh_dir / phys_path.name
            shutil.copy2(str(phys_path), str(dst_path))
            log(f"  Physics: {phys_path.name}")
            file_count += 1

        # -- Part 3: Weapon files ------------------------------------------
        if source_weapon_dir and default_weapon_dir:
            file_count += self._stage_weapons(
                source_weapon_dir, default_weapon_dir,
                content_base, char_id, source_id, target_id, log,
            )

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
        source_slots = self._find_weapon_asset_dirs(source_weapon_dir)
        default_slots = self._find_weapon_asset_dirs(default_weapon_dir)

        if not source_slots:
            # Flat weapon structure: source stores textures directly under
            # Weapons/Texture/ rather than in named slot sub-dirs.
            # Distribute them to every matching default slot.
            return self._stage_flat_weapons(
                source_weapon_dir, default_weapon_dir,
                content_base, source_id, target_id, log,
            )

        common_slots = sorted(set(source_slots.keys()) & set(default_slots.keys()))
        default_only = sorted(set(default_slots.keys()) - set(source_slots.keys()))

        if common_slots:
            log(f"  Weapon slots: {', '.join(common_slots)}")
        if default_only:
            log(f"  Weapon slots (default only, not swapped): {', '.join(default_only)}")

        file_count = 0

        for slot_name in common_slots:
            src_asset_dirs = source_slots[slot_name]
            def_asset_dirs = default_slots[slot_name]

            default_weapons_root = self._find_weapons_root(default_weapon_dir)
            if default_weapons_root is None:
                log("  WARNING: Cannot find Weapons root in default extraction")
                continue

            # Same logic as character body: mesh-swap stages Meshes/ (with
            # uexp raw-patch for MaterialTagAssetUserData), retexture stages
            # Textures/ only.
            wpn_has_meshes = "Meshes" in src_asset_dirs
            wpn_is_retexture = not wpn_has_meshes

            for sub_name, src_sub_dir in src_asset_dirs.items():
                if sub_name.lower() == "materials":
                    continue
                if not wpn_is_retexture and sub_name != "Meshes":
                    continue
                if wpn_is_retexture and not sub_name.lower().startswith("texture"):
                    continue

                if sub_name in def_asset_dirs:
                    rel_path = def_asset_dirs[sub_name].relative_to(default_weapons_root)
                else:
                    any_def_sub = next(iter(def_asset_dirs.values()))
                    rel_path = any_def_sub.parent.relative_to(default_weapons_root) / sub_name

                target_wpn_dir = content_base / "Weapons" / rel_path
                target_wpn_dir.mkdir(parents=True, exist_ok=True)

                for src_path in sorted(src_sub_dir.glob("*")):
                    if not src_path.is_file():
                        continue
                    if src_path.suffix not in (".uasset", ".uexp", ".ubulk"):
                        continue
                    if any(kw in src_path.name for kw in SKIP_FILE_KEYWORDS):
                        continue
                    if any(kw in src_path.name for kw in DEFAULT_PHYSICS_KEYWORDS):
                        continue

                    new_name = src_path.name.replace(source_id, target_id)
                    dst_path = target_wpn_dir / new_name
                    shutil.copy2(str(src_path), str(dst_path))

                    if dst_path.suffix == ".uasset":
                        is_texture_sub = sub_name.lower().startswith("texture")
                        try:
                            modified = patch_skin_id_in_uasset(
                                dst_path, source_id, target_id,
                                skip_texture_refs=(not is_texture_sub),
                                skip_material_refs=True,
                            )
                            if modified:
                                log(f"  [{slot_name}/{sub_name}] Patched: {new_name} ({len(modified)} entries)")
                        except Exception as exc:
                            log(f"  [{slot_name}/{sub_name}] WARNING: Patch failed for {new_name}: {exc}")

                    elif dst_path.suffix == ".uexp" and sub_name == "Meshes":
                        # Raw-patch weapon mesh .uexp to fix MaterialTagAssetUserData
                        # slot FStrings (same reason as character body meshes).
                        try:
                            patch_raw_bytes_in_file(dst_path, source_id, target_id)
                        except Exception as exc:
                            log(f"  [{slot_name}/Meshes] WARNING: uexp patch failed for {new_name}: {exc}")

                    file_count += 1

            # Include default physics for this weapon slot
            if "Meshes" in def_asset_dirs:
                default_mesh_dir = def_asset_dirs["Meshes"]
                def_meshes_rel = default_mesh_dir.relative_to(default_weapons_root)
                physics_target = content_base / "Weapons" / def_meshes_rel
                physics_target.mkdir(parents=True, exist_ok=True)

                for phys_path in sorted(default_mesh_dir.glob("*")):
                    if not phys_path.is_file():
                        continue
                    if not any(kw in phys_path.name for kw in DEFAULT_PHYSICS_KEYWORDS):
                        continue
                    if phys_path.suffix not in (".uasset", ".uexp"):
                        continue
                    dst_path = physics_target / phys_path.name
                    if not dst_path.exists():
                        shutil.copy2(str(phys_path), str(dst_path))
                        file_count += 1

        return file_count

    def _stage_flat_weapons(
        self,
        source_weapon_dir: Path,
        default_weapon_dir: Path,
        content_base: Path,
        source_id: str,
        target_id: str,
        log: LogCallback = _noop_log,
    ) -> int:
        """
        Stage textures from a flat-structured source weapon.

        Retexture skins store weapon textures directly in Weapons/Texture/
        rather than per-slot sub-directories.  This method copies those flat
        textures (renamed source_id → target_id) into every default weapon
        slot that has a matching Texture sub-directory, then includes default
        physics assets as usual.
        """
        src_weapons_root = self._find_weapons_root(source_weapon_dir)
        def_weapons_root = self._find_weapons_root(default_weapon_dir)

        if src_weapons_root is None:
            return 0

        # Find the flat texture folder under the source weapons root.
        src_texture_dir: Optional[Path] = None
        for name in ("Texture", "Textures"):
            candidate = src_weapons_root / name
            if candidate.is_dir():
                src_texture_dir = candidate
                break

        if src_texture_dir is None:
            return 0

        src_files = [
            f for f in sorted(src_texture_dir.glob("*"))
            if f.is_file() and f.suffix in (".uasset", ".uexp", ".ubulk")
            and not any(kw in f.name for kw in SKIP_FILE_KEYWORDS)
        ]
        if not src_files:
            return 0

        default_slots = self._find_weapon_asset_dirs(default_weapon_dir)
        file_count = 0

        if not default_slots:
            # Default also has flat structure — stage directly.
            target_dir = content_base / "Weapons" / src_texture_dir.name
            target_dir.mkdir(parents=True, exist_ok=True)
            log(f"  [flat-wpn → Weapons/{src_texture_dir.name}]")
            for src_path in src_files:
                new_name = src_path.name.replace(source_id, target_id)
                dst_path = target_dir / new_name
                shutil.copy2(str(src_path), str(dst_path))
                if dst_path.suffix == ".uasset":
                    try:
                        modified = patch_skin_id_in_uasset(
                            dst_path, source_id, target_id,
                            skip_texture_refs=False,
                            skip_material_refs=True,
                        )
                        if modified:
                            log(f"    Patched: {new_name} ({len(modified)} entries)")
                    except Exception as exc:
                        log(f"    WARNING: Patch failed for {new_name}: {exc}")
                file_count += 1
            return file_count

        # Slot-based default: fan the flat textures out to each slot.
        for slot_name, sub_dirs in default_slots.items():
            tex_sub: Optional[str] = None
            for name in ("Texture", "Textures"):
                if name in sub_dirs:
                    tex_sub = name
                    break
            if tex_sub is None:
                continue

            target_dir = content_base / "Weapons" / slot_name / tex_sub
            target_dir.mkdir(parents=True, exist_ok=True)
            log(f"  [flat-wpn → {slot_name}/{tex_sub}]")

            for src_path in src_files:
                new_name = src_path.name.replace(source_id, target_id)
                dst_path = target_dir / new_name
                shutil.copy2(str(src_path), str(dst_path))
                if dst_path.suffix == ".uasset":
                    try:
                        modified = patch_skin_id_in_uasset(
                            dst_path, source_id, target_id,
                            skip_texture_refs=False,
                            skip_material_refs=True,
                        )
                        if modified:
                            log(f"    Patched: {new_name} ({len(modified)} entries)")
                    except Exception as exc:
                        log(f"    WARNING: Patch failed for {new_name}: {exc}")
                file_count += 1

            # Include default physics for this slot.
            if "Meshes" in sub_dirs and def_weapons_root is not None:
                default_mesh_dir = sub_dirs["Meshes"]
                def_meshes_rel = default_mesh_dir.relative_to(def_weapons_root)
                physics_target = content_base / "Weapons" / def_meshes_rel
                physics_target.mkdir(parents=True, exist_ok=True)
                for phys_path in sorted(default_mesh_dir.glob("*")):
                    if not phys_path.is_file():
                        continue
                    if not any(kw in phys_path.name for kw in DEFAULT_PHYSICS_KEYWORDS):
                        continue
                    if phys_path.suffix not in (".uasset", ".uexp"):
                        continue
                    dst_path = physics_target / phys_path.name
                    if not dst_path.exists():
                        shutil.copy2(str(phys_path), str(dst_path))
                        file_count += 1

        return file_count

    @staticmethod
    def _find_weapon_asset_dirs(weapon_extract_dir: Path) -> dict[str, dict[str, Path]]:
        slots: dict[str, dict[str, Path]] = {}
        weapons_root = SwapEngine._find_weapons_root(weapon_extract_dir)
        if weapons_root is None:
            return slots

        for slot_dir in sorted(weapons_root.iterdir()):
            if not slot_dir.is_dir():
                continue
            asset_subs: dict[str, Path] = {}
            for sub_dir in sorted(slot_dir.iterdir()):
                if sub_dir.is_dir():
                    has_assets = any(
                        f.suffix in (".uasset", ".uexp", ".ubulk")
                        for f in sub_dir.rglob("*") if f.is_file()
                    )
                    if has_assets:
                        asset_subs[sub_dir.name] = sub_dir
            if asset_subs:
                slots[slot_dir.name] = asset_subs

        return slots

    @staticmethod
    def _find_weapons_root(extract_dir: Path) -> Optional[Path]:
        for dirpath, dirnames, _ in os.walk(extract_dir):
            if "Weapons" in dirnames:
                return Path(dirpath) / "Weapons"
        return None

    @staticmethod
    def _find_skin_root(extract_dir: Path, skin_id: str) -> Optional[Path]:
        for dirpath, dirnames, _ in os.walk(extract_dir):
            if skin_id in dirnames:
                candidate = Path(dirpath) / skin_id
                if "/Characters/" in str(candidate).replace("\\", "/"):
                    return candidate
        return None

    @staticmethod
    def _find_asset_files(base_dir: Path) -> list[Path]:
        result: list[Path] = []
        for root, _dirs, files in os.walk(base_dir):
            for f in files:
                if f.endswith((".uasset", ".uexp", ".ubulk")):
                    result.append(Path(root) / f)
        return sorted(result)

    @staticmethod
    def _cleanup(*dirs: Optional[Path]) -> None:
        for d in dirs:
            if d and d.exists():
                try:
                    shutil.rmtree(d)
                except OSError:
                    pass


