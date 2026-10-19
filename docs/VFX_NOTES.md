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

## Update: in-game retry (Venom Gummy Surprise)

With the fix, cue swaps load: no crash, and some effects come through. Two
gaps remained, both now understood:

- **Missing ability effects.** The default skin's animation montages (e.g.
  `103511_Attack01_MT`) carry per-skin overrides in their notifies
  (`AnimNotifyState_TimedFXWithAO`). Each lists the default asset and the skin's
  one (Niagara systems, Wwise events, emote materials), and the game picks by
  the skin you wear. `swap::skin_overrides` swaps the two names in every
  default-skin package that lists both (29 for Gummy Surprise).
- **Body effects and colors.** These live in the skin's ChildBP: Niagara
  components plus a `User.ChangeColor` parameter. Skin ChildBPs subclass the
  default ChildBP, so one can't replace the other.
  - `MarvelHeroSkinTable` names each skin's ChildBP, sound table and animation
    data. Pointing the default row at the skin's ChildBP should give its body
    effects and colors, with no inheritance loop.
  - Caveats: it's one table shared by every hero, so all swaps must go into one
    combined table mod. Mods outside `/Characters` may also need encryption.
    Not tried yet.

## Update: ultimate cue and skin blueprint (works in-game)

- **Parent cues are no longer held back.** Only a skin cue that imports the
  default cue it would replace is skipped, since it would inherit from itself.
  Gummy Surprise now swaps 7 cues, its ultimate included.
- **Skin table.** `swap::write_skin_table` builds one shared mod
  (`RivalsSwapper_SkinTable`) from every active swap that has a blueprint. In
  `MarvelHeroSkinTable` it swaps the default and skin ChildBP names, path and
  `_C` class, so the default row loads the skin's ChildBP. It's rebuilt after
  every swap and removal.
- **Still open:** the skin-id-built buff cues (`Cue_Buff_10354101_302_BP`,
  `Cue_Buff_10356101_302_BP`), which nothing references by name.

## Variant cues (`<base>_<NNN>_BP`): not swappable yet

Status: **both attempts failed in-game and were reverted.** Affected skins are
flagged in the app (`Skin::missing_effects`: "some VFX stay default").

**What they are**

- 753 skin cues are variants: `Cue_Buff_10354101_302_BP` subclasses the
  default's `Cue_Buff_10354101_BP`. Default skins have none.
- Nothing references them by name, so the game builds the name at runtime. The
  logic is in NetEase's encrypted Python archive (`script.uep`, game root and
  `Content/Marvel/NonAssetsToCopy`), which we don't decrypt.
- **The variant number.** Each `MarvelHeroSkinTable` row has a string field
  ending in the skin id (sometimes prefixed, e.g. `ps1035304`), followed by two
  numbers. The cue suffix is the first, or the second for recolors:

  | row | numbers |
  |---|---|
  | default 1035001 | 1, 1 ("no variant") |
  | 1035302 | 302, 1 |
  | 1035304 (recolor) | 304, 302 |

- They can't be moved onto the base cue's path: they subclass it, so they would
  inherit from themselves.

**Attempt A: copy each variant cue into the default folder under the same name**
(`1035001/Cues/103541/Cue_Buff_10354101_302_BP`)

- Result: no effect in-game. The default row's variant number is 1, so the game
  never asks for a `_302` cue.

**Attempt B: A, plus copy the skin row's two numbers into the default row**

- The patch is in-place, 8 bytes, with no size change, found by matching the id
  string followed by two numbers in 1..=999. It paired 693 of 697 skins.
- Result: **skins broke entirely in-game.** The numbers clearly drive more than
  cue names, likely model, material and other per-variant lookups that then
  point at assets the default skin doesn't have.

**Ideas for later**

- Find out what else the variant number selects before touching it again: scan
  for other `_<NNN>` assets keyed by it, beyond cues.
- Flatten a variant cue into a standalone copy of the base with the variant's
  overrides, then put it on the base's path. That needs the base's and the
  variant's property data merged; UAssetTool `to_json` with an up-to-date
  `.usmap` might make this practical.
