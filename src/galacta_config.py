"""
Builds a Project Galacta Skin Config: a cooked instance of
/Game/Marvel/ProjectGalacta/Blueprints/GAL_ModSkinConfig (shipped by the
Galacta core mod), written through UAssetTool from_json.

The game never loads this asset; only the Galacta Companion App reads it when
building its manifest, so plain tagged properties are used.
"""

import json
import subprocess
from pathlib import Path

from .uassettool_wrapper import UASSETTOOL_EXE, _SUBPROCESS_FLAGS

GALACTA_ROOT = "/Game/Marvel/ProjectGalacta"
_BP_PACKAGE = f"{GALACTA_ROOT}/Blueprints/GAL_ModSkinConfig"
_API = "UAssetAPI"


def galacta_mod_dir(hero_id: str, skin_id: str, mod_folder: str) -> str:
    return f"{GALACTA_ROOT}/Meshes/{hero_id}/{skin_id}/{mod_folder}"


def _import(obj: str, cls: str, cls_pkg: str, outer: int) -> dict:
    return {
        "$type": f"{_API}.Import, {_API}",
        "ObjectName": obj, "OuterIndex": outer,
        "ClassPackage": cls_pkg, "ClassName": cls,
        "PackageName": "None", "bImportOptional": False,
    }


def _prop(kind: str, name: str, value) -> dict:
    return {
        "$type": f"{_API}.PropertyTypes.Objects.{kind}PropertyData, {_API}",
        "Name": name, "ArrayIndex": 0, "IsZero": False,
        "PropertyTagFlags": "None", "PropertyTagExtensions": "NoExtension",
        "Value": value,
    }


def build_config_json(
    config_name: str,
    package_path: str,
    hero_id: int,
    skin_id: int,
    skin_name: str,
    author: str,
    mesh_package_path: str,
) -> dict:
    mesh_name = mesh_package_path.rsplit("/", 1)[1]
    imports = [
        _import(_BP_PACKAGE, "Package", "/Script/CoreUObject", 0),                      # -1
        _import("GAL_ModSkinConfig_C", "BlueprintGeneratedClass", "/Script/Engine", -1),  # -2
        _import("Default__GAL_ModSkinConfig_C", "GAL_ModSkinConfig_C", _BP_PACKAGE, -1),  # -3
        _import(mesh_package_path, "Package", "/Script/CoreUObject", 0),               # -4
        _import(mesh_name, "SkeletalMesh", "/Script/Engine", -4),                      # -5
    ]
    data = [
        _prop("Int", "TargetHeroID", hero_id),
        _prop("Int", "TargetSkinID", skin_id),
        _prop("Str", "SkinName", skin_name),
        _prop("Str", "Author", author),
        _prop("Object", "MeshToSwap", -5),
    ]
    export = {
        "$type": f"{_API}.ExportTypes.NormalExport, {_API}",
        "Data": data,
        "ObjectGuid": None,
        "SerializationControl": "NoExtension",
        "Operation": "None",
        "ObjectName": config_name,
        "OuterIndex": 0, "ClassIndex": -2, "SuperIndex": 0, "TemplateIndex": -3,
        "ObjectFlags": "RF_Public, RF_Standalone, RF_Transactional",
        "SerialSize": 0, "SerialOffset": 0,
        "ScriptSerializationStartOffset": 0, "ScriptSerializationEndOffset": 0,
        "bForcedExport": False, "bNotForClient": False, "bNotForServer": False,
        "PackageGuid": "{00000000-0000-0000-0000-000000000000}",
        "IsInheritedInstance": False,
        "PackageFlags": "PKG_None",
        "bNotAlwaysLoadedForEditorGame": False,
        "bIsAsset": True,
        "GeneratePublicHash": False,
        "SerializationBeforeSerializationDependencies": [],
        "CreateBeforeSerializationDependencies": [-5],
        "SerializationBeforeCreateDependencies": [-2, -3],
        "CreateBeforeCreateDependencies": [],
        "Extras": "AAAAAA==",
    }
    names = {
        "/Script/CoreUObject", "/Script/Engine", "Package", "BlueprintGeneratedClass",
        "SkeletalMesh", "IntProperty", "StrProperty", "ObjectProperty", "None",
        "TargetHeroID", "TargetSkinID", "SkinName", "Author", "MeshToSwap",
        "GAL_ModSkinConfig_C", "Default__GAL_ModSkinConfig_C", _BP_PACKAGE,
        mesh_package_path, mesh_name, package_path, config_name,
    }
    # UAssetAPI stores "Foo_123" as FName("Foo", 124), so the base must exist too.
    for n in list(names):
        base, _, num = n.rpartition("_")
        if base and num.isdigit() and not (len(num) > 1 and num.startswith("0")):
            names.add(base)
    names = sorted(names)
    engine = {"$type": f"{_API}.FEngineVersion, {_API}", "Major": 5, "Minor": 3,
              "Patch": 2, "Changelist": 0, "Branch": None}
    return {
        "$type": f"{_API}.UAsset, {_API}",
        "NameMap": names,
        "CustomSerializationFlags": "None",
        "UseSeparateBulkDataFiles": True,
        "LegacyFileVersion": -8,
        "WillSerializeNameHashes": True,
        "DataResourceVersion": "Invalid",
        "AppendedNullBytes": 0,
        "OtherAssetsFailedToAccess": [],
        "SoftObjectPathList": [],
        "BulkData": "",
        "AssetRegistryData": "AAAAAA==",  # zero registry objects
        "doWeHaveAssetRegistryData": True,
        "doWeHaveWorldTileInfo": False,
        "PayloadTocOffset": -1,
        "AdditionalPackagesToCook": [],
        "RecordedEngineVersion": engine,
        "RecordedCompatibleWithEngineVersion": engine,
        "Generations": [{"$type": f"{_API}.FGenerationInfo, {_API}",
                         "ExportCount": 1, "NameCount": len(names)}],
        "IsUnversioned": True,
        "FileVersionLicenseeUE": 0,
        "ObjectVersion": "VER_UE4_CORRECT_LICENSEE_FLAG",
        "ObjectVersionUE5": "DATA_RESOURCES",
        "CustomVersionContainer": [],
        "Exports": [export],
        "Imports": imports,
        "DependsMap": [[]],
        "SoftPackageReferenceList": [],
        "PackageGuid": "{00000000-0000-0000-0000-000000000000}",
        "PackageSource": 0,
        "ChunkIDs": [],
        "FolderName": package_path,
        "PackageFlags": "PKG_Cooked, PKG_FilterEditorOnly",
        "HasUnversionedProperties": False,
        "IsFilterEditorOnly": True,
        "UsesEventDrivenLoader": True,
    }


def write_skin_config(
    content_dir: Path,
    hero_id: str,
    skin_id: str,
    mod_folder: str,
    skin_name: str,
    author: str,
    mesh_package_path: str,
) -> Path:
    """
    Write <content_dir>/Marvel/ProjectGalacta/Meshes/{hero}/{skin}/{mod}/GAL_SkinConfig.uasset
    (content_dir is the staging dir's Marvel/Content). Raises RuntimeError on failure.
    """
    config_name = "GAL_SkinConfig"
    package_path = f"{galacta_mod_dir(hero_id, skin_id, mod_folder)}/{config_name}"
    out = content_dir / (package_path.removeprefix("/Game/") + ".uasset")
    out.parent.mkdir(parents=True, exist_ok=True)

    json_path = out.with_suffix(".json")
    json_path.write_text(json.dumps(build_config_json(
        config_name, package_path, int(hero_id), int(skin_id),
        skin_name, author, mesh_package_path,
    )), encoding="utf-8")

    try:
        result = subprocess.run(
            [str(UASSETTOOL_EXE), "from_json", str(json_path), str(out)],
            capture_output=True, text=True, timeout=120,
            creationflags=_SUBPROCESS_FLAGS,
        )
    finally:
        json_path.unlink(missing_ok=True)
    if not out.exists():
        raise RuntimeError(f"UAssetTool from_json failed:\n{result.stdout}\n{result.stderr}")
    return out
