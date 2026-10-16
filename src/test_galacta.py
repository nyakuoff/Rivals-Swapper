"""Self-check for Galacta config generation + package relocation. Run: python -m src.test_galacta"""

import json
import subprocess
import tempfile
from pathlib import Path

from .galacta_config import write_skin_config
from .uasset_patcher import relocate_package
from .uassettool_wrapper import UASSETTOOL_EXE


def _read(uasset: Path) -> dict:
    out = uasset.parent / "json"
    subprocess.run([str(UASSETTOOL_EXE), "to_json", str(uasset), "", str(out)],
                   capture_output=True, check=True)
    return json.loads((out / f"{uasset.stem}.json").read_text(encoding="utf-8-sig"))


def demo() -> None:
    mesh_pkg = "/Game/Marvel/ProjectGalacta/Meshes/1048/1048001/Mod/SK_1048_1048309"
    with tempfile.TemporaryDirectory() as tmp:
        cfg = write_skin_config(Path(tmp), "1048", "1048001", "Mod", "Skin", "Me", mesh_pkg)
        d = _read(cfg)
        props = {p["Name"]: p["Value"] for p in d["Exports"][0]["Data"]}
        assert props == {"TargetHeroID": 1048, "TargetSkinID": 1048001, "SkinName": "Skin",
                         "Author": "Me", "MeshToSwap": -5}, props
        assert d["Imports"][3]["ObjectName"] == mesh_pkg

        # Relocate to a longer path; offsets must stay consistent for UAssetAPI to re-read it.
        new_path = "/Game/Marvel/ProjectGalacta/Meshes/1048/1048001/AMuchLongerModFolder/GAL_SkinConfig"
        old = relocate_package(cfg, new_path)
        assert old.endswith("/Mod/GAL_SkinConfig"), old
        d2 = _read(cfg)
        assert d2["FolderName"] == new_path
        assert new_path in d2["NameMap"]
        assert {p["Name"]: p["Value"] for p in d2["Exports"][0]["Data"]} == props
    print("OK")


if __name__ == "__main__":
    demo()
