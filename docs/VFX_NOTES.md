# VFX swap: research notes

Status: **not shipped.** Two attempts were tested in-game and both failed. The
code was removed. These notes are here so a future attempt starts from what we
already know.

## Where skin VFX live

- Each skin's ability effects are gameplay cue blueprints at
  `/Game/Marvel/Characters/{hero}/{skin}/Cues/<rel>`. The default skin has the
  same layout under `{hero}001/Cues/<rel>`.
- A skin cue and the default cue at the same `<rel>` carry the same gameplay tag
  (e.g. `GameplayCue.Scope.HitImpact.10352101`). In the one pair we compared
  (Venom 1035302 vs 1035001 `Cue_Scope_HitImpact_10352101_BP`), the only
  differences were the package path and the Niagara system it spawns.
- No DataTable maps skin → cue. The game appears to pick the skin's cue by path
  or tag at runtime.
- There are about 625 skin cues across 225 skins that share a relative path with
  a default cue:
  - About 251 of them **subclass** the default cue at the same path. These can't
    simply replace it: the replacement would import itself.
  - Many default cues are parents for other skins' cues, and some for other
    heroes' cues (e.g. hero 4019's cues subclass hero 1028's; heroes 1055, 1064
    and 1066 use 1025's curves).
- Cue parents are NetEase's Python-generated classes, e.g.
  `/Script/UnrealEnginePython/subclassing/heroes/c1035/ability/PyAbility.PyCue_HitImpact`.
- Some effects aren't cues at all. They're attached to the body or ChildBP, like
  Venom's web.

## Attempt 1: relocate cues through UAssetTool (like meshes)

Result: invisible VFX, and the game usually crashes.

- The crash is an `EXCEPTION_ACCESS_VIOLATION` in the zen async loader
  (`Marvel-Win64-Shipping+0x108b22a`; strings nearby: `AsyncLoadingThread`,
  `Event_ProcessExportBundle`).
- What that code does: it walks an export's dependency bundle. For an import, it
  checks the import store, and if the object isn't found it indexes
  `ImportedAsyncPackages[(importMapEntry >> 32) & 0x3fffffff]`. A script import
  that can't be resolved lands there with a garbage index.
- Cause: the script-object hashes of the Python classes **can't be recomputed
  from their names.**
  - The game's super class hash is `2B3B40382EE6A1C1`. The standard hash of the
    name retoc reconstructs is `C7C99459589CFD`.
  - The legacy round trip (retoc zen→legacy, then UAssetTool legacy→zen) writes
    the standard hash, which the game can't resolve.
  - UAssetTool also puts script imports into dependency bundles, which the
    game's own cooker never does.
- Meshes and sound tables work because they have no Python-class imports.
- Parent-class replacement was ruled out as the crash cause. Excluding parent
  cues didn't change the crash.

## Attempt 2: zen-level relocation in a separate container

Result: no crash, but **no VFX were swapped in-game.**

- Method: copy the game's raw zen bytes and replace only the package-name entry
  in the name map. Re-serialize with retoc's `FZenPackageHeader::serialize`. The
  round trip is byte-exact on unmodified packages.
- The output went into its own container, `{mod}_FX_9999999_P`, written with
  `retoc::iostore_writer::IoStoreWriter` plus an empty V11 .pak (as retoc
  `to-zen` does).
- Cues that other cues import (parents) were skipped. That left 207 of 625
  cues. Venom 1035302 kept 6 of 7.
- The packages read back fine with retoc. So the game either never used them,
  or resolves cues another way.

## Open questions and ideas for next time

- **How does the game choose a skin's cue?** Likely somewhere in the Python
  scripts (`PyRuntim` section, `UnrealEnginePython`).
  - It may pick by skin id and not by path. Then a default-path override is
    never read, which would explain attempt 2.
  - Find the lookup before building anything else.
- **Was the FX container even mounted?** Check whether the UTOC bypass loads a
  second retoc-written container. A test: put a known-visible change (e.g. a
  mesh) in a retoc-written container.
- **Asset registry.** Cues may be discovered through `AssetRegistry.bin` by tag.
  A swapped package at the default path may need a matching registry entry, or
  the game may keep using the skin cue's entry.
- **Swap the Niagara systems instead of the cues.** Overriding the default
  `NS_*` packages with the skin's (the paths differ only by skin id) avoids the
  Python-class problem entirely. Niagara packages have no Python imports, so the
  normal UAssetTool path should work. This looks like the most promising lead.
- **Subclassing cues** (about 251) would still need flattening or a different
  approach.

## Update: the Python-class problem is solved

The "hash can't be recomputed" theory above was wrong.

- **What the hash really is.** The game's hash is the standard one, taken over
  the full name: `…/PyAbility_103521.PyCue_HitImpact_10352101` →
  `2B3B40382EE6A1C1`. All 57,874 Python script objects in the global table hash
  correctly this way.
- **Where it broke.** Unreal stores such names as a base name plus a number
  (`PyCue_HitImpact` + `10352102`). retoc's legacy output keeps the numbers.
  **UAssetTool drops FName numbers when it hashes script imports**, so it writes
  the hash of `…/PyAbility.PyCue_HitImpact`, a class that doesn't exist.
- **The fix.** `swap::spell_out_script_import_numbers` runs on every package
  before UAssetTool packs it. It appends the full names to the name map and
  points the script imports at them with number 0, so UAssetTool hashes the
  right string.
- **How it was checked.**
  - `python_class_imports_survive_packing` covers the cue that crashed the game:
    its imports mismatch without the fix and all match with it.
  - A survey packed all 5,221 cue and MVP packages: 5,216 keep every script
    import exactly.
  - The other 5 MVPs import script objects that aren't in the game's own table
    (stale leftovers). UAssetTool leaves those out.

This likely explains attempt 1 (crash and invisible effects). Cue swaps should
be retried with this fix before trying anything else. The unanswered questions
from attempt 2 (how the game picks a skin's cue) only matter if the retry still
shows no effects.
