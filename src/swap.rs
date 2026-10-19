//! Swap pipeline: extract the source skin's mesh, move it onto the default
//! skin's own mesh path (package and object renamed to match), then pack it
//! into ~mods with UAssetTool (which applies the Rivals-specific fixes). The game
//! then draws it whenever the default skin is equipped; nothing runs in-game.
//! Mods under /Game/Marvel/Characters load as-is with the community UTOC
//! signature bypass, so no encryption is needed.
//!
//! Retexture skins have no mesh: their ChildBP inherits a mesh skin's ChildBP
//! and overrides its material slots. Those are swapped as a copy of the parent
//! mesh whose material imports point at the retexture's (vanilla) materials.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

use crate::command;
use crate::db::{Character, Skin};
use crate::game::Game;
use crate::settings::SwapRecord;

pub fn mod_files(mods_dir: &Path, mod_name: &str) -> [PathBuf; 3] {
    ["pak", "utoc", "ucas"].map(|ext| mods_dir.join(format!("{mod_name}_9999999_P.{ext}")))
}

/// What a swap brings along besides the model.
#[derive(Clone, Copy)]
pub struct SwapOptions {
    /// The skin's own ability sound effects (when it has them).
    pub sounds: bool,
    /// The skin's own ability VFX (gameplay cues, when it has them).
    pub effects: bool,
    /// The default MVP plays the skin's own MVP, whole scene (when it ships one),
    /// through the shared MVP table.
    pub mvp: bool,
}

fn safe_mod_name(name: &str) -> String {
    name.replace(' ', "_")
        .chars()
        .filter(|c| !r#"<>:"/\|?*"#.contains(*c))
        .collect()
}

pub fn create(
    game: &Game,
    work_dir: &Path,
    mods_dir: &Path,
    character: &Character,
    skin: &Skin,
    options: SwapOptions,
    log: &dyn Fn(String),
) -> Result<SwapRecord> {
    let (char_id, source_id, target_id) =
        (&character.char_id, &skin.id, &character.default_skin_id);
    let mod_name = safe_mod_name(&format!("{}_{}", character.name, skin.name));

    let _ = std::fs::remove_dir_all(work_dir);
    let result = (|| {
        let index = game.skins();
        let retexture = if index.meshes.contains_key(source_id) {
            None
        } else {
            log(format!(
                "{} is a recolor, finding its base mesh...",
                skin.name
            ));
            Some(retexture_source(
                game,
                &work_dir.join("bp"),
                char_id,
                source_id,
            )?)
        };
        let mesh_skin = retexture.as_ref().map_or(source_id, |r| &r.base_skin);
        let meshes = index
            .meshes
            .get(mesh_skin)
            .context("This skin's mesh wasn't found in the game files.")?;
        let targets = index
            .meshes
            .get(target_id)
            .context("The default skin's mesh wasn't found in the game files.")?;

        let content_dir = work_dir.join("staging/Marvel/Content");
        let none = vec![];
        let lobby_meshes = index.lobby.get(mesh_skin).unwrap_or(&none);
        let lobby_targets = index.lobby.get(target_id).unwrap_or(&none);
        // In-game meshes first, then the lobby (menu) models. Multi-form heroes:
        // each form's mesh replaces the default mesh of that form.
        let mut packed = vec![];
        for (sources, targets, lobby) in [
            (meshes, targets, false),
            (lobby_meshes, lobby_targets, true),
        ] {
            for (form, package) in sources {
                let Some((_, target)) = targets.iter().find(|(f, _)| f == form) else {
                    if !lobby {
                        log(format!(
                            "  Skipping form {form}: the default skin has no mesh for it"
                        ));
                    }
                    continue;
                };
                let mesh_name = package.rsplit('/').next().unwrap_or(package);
                let renames = match &retexture {
                    Some(r) if lobby && r.lobby_materials.is_empty() => {
                        log("  Lobby model kept as is: this recolor has no lobby materials".into());
                        continue;
                    }
                    _ => {
                        log(format!(
                            "Extracting {mesh_name}{}...",
                            if *form > 0 {
                                format!(" (form {form})")
                            } else {
                                String::new()
                            }
                        ));
                        let mesh = game.extract_package(package, &work_dir.join("extract"))?;
                        let renames = match &retexture {
                            Some(r) => {
                                let materials = if lobby {
                                    &r.lobby_materials
                                } else {
                                    &r.materials
                                };
                                let renames = material_renames(&names(&mesh)?, materials, lobby);
                                ensure!(
                                    lobby || !renames.is_empty() || meshes.len() > 1,
                                    "None of {}'s materials match the slots of {mesh_name}.",
                                    skin.name
                                );
                                log(format!(
                                    "  {} of {} materials applied",
                                    renames.len() / 2,
                                    materials.len()
                                ));
                                renames
                            }
                            None => HashMap::new(),
                        };
                        stage(&mesh, &content_dir, target)?;
                        renames
                    }
                };
                let dst = content_dir.join(target.trim_start_matches("/Game/"));
                let old = relocate_package(&dst.with_extension("uasset"), target, &renames)?;
                log(format!(
                    "  {}: {old} -> {target}",
                    if lobby { "Lobby" } else { "Mesh" }
                ));
                if !lobby {
                    packed.push((*form, target.clone()));
                }
            }
        }
        ensure!(
            !packed.is_empty(),
            "The default skin has no mesh for any of this skin's forms."
        );

        // Sound tables: the skin's own, or for a recolor its base skin's, each
        // replacing the default skin's table of the same kind.
        let sound_skin = Some(source_id)
            .filter(|s| index.sound_tables.contains_key(*s))
            .or_else(|| {
                retexture
                    .as_ref()
                    .map(|r| &r.base_skin)
                    .filter(|b| *b != target_id && index.sound_tables.contains_key(*b))
            })
            .filter(|_| options.sounds);
        if let (Some(tables), Some(defaults)) = (
            sound_skin.and_then(|s| index.sound_tables.get(s)),
            index.sound_tables.get(target_id),
        ) {
            let mut kinds: Vec<_> = tables.iter().collect();
            kinds.sort();
            for (kind, package) in kinds {
                let Some(target) = defaults.get(kind) else {
                    continue;
                };
                let table = game.extract_package(package, &work_dir.join("extract"))?;
                let dst = stage(&table, &content_dir, target)?;
                let old = relocate_package(&dst, target, &HashMap::new())?;
                log(format!("  Sounds: {old} -> {target}"));
            }
        }

        // Ability VFX: the skin's gameplay cues (or for a recolor, its base
        // skin's) replace the default skin's cues at the same paths.
        let cue_skin = Some(source_id)
            .filter(|s| index.cues.contains_key(*s))
            .or_else(|| {
                retexture
                    .as_ref()
                    .map(|r| &r.base_skin)
                    .filter(|b| *b != target_id && index.cues.contains_key(*b))
            })
            .filter(|_| options.effects);
        let cues = cue_skin.map_or_else(Vec::new, |s| {
            swappable_cues(game, &work_dir.join("cues"), s, target_id)
        });
        for (package, target) in &cues {
            let cue = game.extract_package(package, &work_dir.join("extract"))?;
            let dst = stage(&cue, &content_dir, target)?;
            relocate_package(&dst, target, &HashMap::new())?;
        }
        if !cues.is_empty() {
            log(format!("  Effects: {} ability cues", cues.len()));
        }

        // Default-skin animations name per-skin overrides (effects, sounds and
        // materials in anim notifies): the default entry takes the skin's asset.
        let override_skin = retexture.as_ref().map_or(source_id, |r| &r.base_skin);
        let overrides = skin_overrides(
            game,
            &work_dir.join("overrides"),
            override_skin,
            target_id,
            options,
        );
        let overrides: Vec<_> = overrides
            .into_iter()
            .filter(|(p, _)| {
                !content_dir
                    .join(p.trim_start_matches("/Game/"))
                    .with_extension("uasset")
                    .exists()
            })
            .collect();
        for (package, renames) in &overrides {
            let file = game.extract_package(package, &work_dir.join("extract"))?;
            let dst = stage(&file, &content_dir, package)?;
            relocate_package(&dst, package, renames)?;
        }
        if !overrides.is_empty() {
            log(format!(
                "  Animation overrides: {} packages",
                overrides.len()
            ));
        }

        log("Packing IoStore mod into ~mods (UAssetTool)...".into());
        let [pak, utoc, ucas] = pack(mods_dir, &mod_name, &content_dir)?;
        // The skin's blueprint (body effects, effect colors) can take the default
        // skin's place through the shared skin table; see `write_skin_table`.
        let childbp =
            options.effects && source_id != target_id && index.blueprints.contains(source_id);
        Ok(SwapRecord {
            skin_id: source_id.clone(),
            skin_name: skin.name.clone(),
            mod_name,
            char_id: char_id.clone(),
            pak_path: pak,
            utoc_path: utoc,
            ucas_path: ucas,
            target_skin_id: target_id.clone(),
            meshes: packed,
            childbp,
            mvp: options.mvp,
        })
    })();
    let _ = std::fs::remove_dir_all(work_dir);
    result
}

/// Default-skin packages (animations mostly) whose notifies list a per-skin
/// override for `skin` next to the default asset, with the renames that swap
/// the two: the default entry loads the skin's asset (and the skin's entry the
/// default one). Effects and sounds only when the options ask for them.
fn skin_overrides(
    game: &Game,
    work: &Path,
    skin: &str,
    default: &str,
    options: SwapOptions,
) -> Vec<(String, HashMap<String, String>)> {
    use rayon::prelude::*;
    if skin == default {
        return vec![];
    }
    let prefix = format!("/Game/Marvel/Characters/{}/{default}/", &default[..4]);
    let mut out: Vec<(String, HashMap<String, String>)> = game
        .packages_under(&prefix)
        .par_iter()
        .filter(|package| {
            // Cheap first pass on the zen header: only packages naming something
            // in the skin's folder get the full extraction below.
            let own = format!("/{skin}/");
            game.header_names(package)
                .is_ok_and(|n| n.iter().any(|n| n.contains(&own)))
        })
        .filter_map(|package| {
            let work = work.join(format!("{:?}", std::thread::current().id()));
            let names = names(&game.extract_package(package, &work).ok()?).ok()?;
            let all: HashSet<&String> = names.iter().collect();
            let mut renames = HashMap::new();
            for own in names.iter().filter(|n| n.contains(&format!("/{skin}/"))) {
                let wanted = if own.contains("/Wwise") {
                    options.sounds
                } else if own.contains("/VFX/") {
                    options.effects
                } else {
                    true
                };
                let base = own
                    .replace(&format!("/{skin}/"), &format!("/{default}/"))
                    .replace(skin, default);
                if !wanted || !all.contains(&base) {
                    continue;
                }
                // The object names differ too when they carry the skin id.
                let (own_name, base_name) = (own.rsplit('/').next()?, base.rsplit('/').next()?);
                let mut pairs = vec![(base.clone(), own.clone())];
                if own_name != base_name
                    && all.contains(&own_name.to_string())
                    && all.contains(&base_name.to_string())
                {
                    pairs.push((base_name.to_string(), own_name.to_string()));
                }
                for (a, b) in pairs {
                    renames.insert(b.clone(), a.clone());
                    renames.insert(a, b);
                }
            }
            (!renames.is_empty()).then(|| (package.clone(), renames))
        })
        .collect();
    let _ = std::fs::remove_dir_all(work);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The skin's cues that can replace the default skin's (package, default path).
/// A skin cue that subclasses (imports) the default cue it would replace is left
/// out: moved onto that path it would inherit from itself.
pub fn swappable_cues(
    game: &Game,
    work: &Path,
    skin: &str,
    default: &str,
) -> Vec<(String, String)> {
    let index = game.skins();
    let (Some(own), Some(defaults)) = (index.cues.get(skin), index.cues.get(default)) else {
        return vec![];
    };
    let mut out: Vec<(String, String)> = own
        .iter()
        .filter_map(|(rel, p)| {
            let target = defaults.get(rel)?;
            let names = names(&game.extract_package(p, work).ok()?).ok()?;
            (!names.contains(target)).then(|| (p.clone(), target.clone()))
        })
        .collect();
    out.sort();
    out
}

/// Pack the staging tree into `{mod_name}_9999999_P` in ~mods with UAssetTool.
/// Returns the (pak, utoc, ucas) paths that were written.
fn pack(mods_dir: &Path, mod_name: &str, content_dir: &Path) -> Result<[String; 3]> {
    for f in crate::walk(content_dir)
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "uasset"))
    {
        spell_out_script_import_numbers(f)?;
    }
    std::fs::create_dir_all(mods_dir)?;
    let files = mod_files(mods_dir, mod_name);
    let out = command(crate::uassettool())
        .arg("create_mod_iostore")
        .arg(mods_dir.join(format!("{mod_name}_9999999_P")))
        .arg(content_dir)
        .output()
        .context("running UAssetTool")?;
    // UAssetTool often exits non-zero on harmless warnings; trust the output files.
    if !files[1].exists() || !files[2].exists() {
        for f in &files {
            let _ = std::fs::remove_file(f);
        }
        bail!(
            "UAssetTool create_mod_iostore failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(files.map(|p| {
        if p.exists() {
            p.to_string_lossy().into_owned()
        } else {
            String::new()
        }
    }))
}

/// The shared skin-table mod: one for every swap, since all heroes share the table.
pub const SKIN_TABLE_MOD: &str = "RivalsSwapper_SkinTable";
const SKIN_TABLE: &str = "/Game/Marvel/Data/DataTable/MarvelHeroSkinTable";

/// Rewrite the skin-table mod for these (default skin, skin) pairs: each default
/// skin's row takes the skin's ChildBP, which holds its body effects and the
/// colors its effects use. Skin ChildBPs subclass the default one, so they
/// can't be moved onto its path; pointing the row at them works instead. The
/// skin's own row gets the default ChildBP (the names are swapped). No pairs:
/// the mod is removed.
pub fn write_skin_table(
    game: &Game,
    work_dir: &Path,
    mods_dir: &Path,
    pairs: &[(String, String)],
) -> Result<usize> {
    let bp = |s: &str| {
        [
            format!("/Game/Marvel/Characters/{}/{s}/{s}_ChildBP", &s[..4]),
            format!("{s}_ChildBP_C"),
        ]
    };
    let groups = pairs
        .iter()
        .map(|(default, skin)| bp(default).into_iter().zip(bp(skin)).collect())
        .collect();
    write_table_mod(
        game,
        work_dir,
        mods_dir,
        SKIN_TABLE_MOD,
        SKIN_TABLE,
        groups,
        &|_, _| {},
    )
}

/// The shared MVP-table mod, like the skin table's.
pub const MVP_TABLE_MOD: &str = "RivalsSwapper_MVPTable";
const MVP_TABLE: &str = "/Game/Marvel/Data/DataTable/UI/HeroSkin/UIHeroMVPTable";

/// What a hero's default MVP plays, in the shared MVP-table mod.
pub enum MvpChoice {
    /// The default MVP's row and this MVP's trade keys: the default MVP id plays
    /// the whole MVP (sequence, stage, final frame). A special MVP's row only has
    /// sequences for the skins it was made for, so the first of `skins` that it
    /// has is relabeled as `default_skin` (else the default skin gets nothing
    /// and the MVP plays black).
    Special {
        default_id: String,
        mvp_id: String,
        default_skin: String,
        skins: Vec<String>,
    },
    /// In the default MVP's row, the default skin's sequence and this skin's
    /// trade places: the default skin plays the skin's version of the MVP.
    SkinVersion {
        default_sequence: String,
        skin_sequence: String,
    },
}

/// Rewrite the MVP-table mod for these choices, or remove it when there are none.
pub fn write_mvp_table(
    game: &Game,
    work_dir: &Path,
    mods_dir: &Path,
    choices: &[MvpChoice],
) -> Result<usize> {
    let object = |p: &str| p.rsplit('/').next().unwrap_or_default().to_string();
    let groups = choices
        .iter()
        .map(|c| match c {
            MvpChoice::Special {
                default_id, mvp_id, ..
            } => vec![(default_id.clone(), mvp_id.clone())],
            MvpChoice::SkinVersion {
                default_sequence: d,
                skin_sequence: s,
            } => {
                vec![(d.clone(), s.clone()), (object(d), object(s))]
            }
        })
        .collect();
    let relabel = |names: &[String], data: &mut [u8]| {
        for c in choices {
            let MvpChoice::Special {
                mvp_id,
                default_skin,
                skins,
                ..
            } = c
            else {
                continue;
            };
            let Some(row) = table_row(names, data, mvp_id) else {
                continue;
            };
            // Same length (skin ids are 7 digits), so the patch is in place.
            let Some((at, len)) = skins.iter().find_map(|s| {
                let pat = fstring(s);
                data[row.clone()]
                    .windows(pat.len())
                    .position(|w| w == pat.as_slice())
                    .map(|i| (row.start + i, pat.len()))
            }) else {
                continue;
            };
            let new = fstring(default_skin);
            if new.len() == len {
                data[at..at + len].copy_from_slice(&new);
            }
        }
    };
    write_table_mod(
        game,
        work_dir,
        mods_dir,
        MVP_TABLE_MOD,
        MVP_TABLE,
        groups,
        &relabel,
    )
}

/// The byte range of the table row keyed `id` in its export data: from the
/// key's name reference to the next row key's (keys are all-digit names of the
/// same length, like MVP or emote ids).
pub(crate) fn table_row(names: &[String], data: &[u8], id: &str) -> Option<std::ops::Range<usize>> {
    let u32_at = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap()) as usize;
    let is_key = |o: usize| {
        let v = u32_at(o);
        v < names.len()
            && u32_at(o + 4) == 0
            && names[v].len() == id.len()
            && names[v].bytes().all(|c| c.is_ascii_digit())
    };
    let index = names.iter().position(|n| n == id)?;
    let start =
        (0..data.len().checked_sub(8)?).find(|&o| u32_at(o) == index && u32_at(o + 4) == 0)?;
    let end = (start + 8..data.len() - 8)
        .find(|&o| is_key(o))
        .unwrap_or(data.len());
    Some(start..end)
}

/// The shared hero-gallery table mod, like the skin table's.
pub const GALLERY_TABLE_MOD: &str = "RivalsSwapper_GalleryTable";
const GALLERY_TABLE: &str = "/Game/Marvel/Data/DataTable/HeroGallery/UISkinTable";

/// Rewrite the hero-gallery table mod for these (default skin, skin) pairs: the
/// default skin's row takes the skin's show and pose actors (`ShowBP`,
/// `LikeBP`). A skin's MVP sequences bind its own show actor, so an MVP of that
/// skin played on the default skin needs them (else its final shot is empty).
pub fn write_gallery_table(
    game: &Game,
    work_dir: &Path,
    mods_dir: &Path,
    pairs: &[(String, String)],
) -> Result<usize> {
    let actors = |s: &str| {
        let dir = format!("/Game/Marvel/Characters/{}/{s}", &s[..4]);
        ["ShowBP", "LikeBP"]
            .map(|a| [format!("{dir}/{s}_{a}"), format!("{s}_{a}_C")])
            .concat()
    };
    let groups = pairs
        .iter()
        .map(|(default, skin)| actors(default).into_iter().zip(actors(skin)).collect())
        .collect();
    write_table_mod(
        game,
        work_dir,
        mods_dir,
        GALLERY_TABLE_MOD,
        GALLERY_TABLE,
        groups,
        &|_, _| {},
    )
}

/// The lobby emote table mod (shared by all heroes), and the per-hero emote
/// table mods (`<prefix><hero>`).
pub const EMOTE_UI_TABLE_MOD: &str = "RivalsSwapper_EmoteUITable";
const EMOTE_UI_TABLE: &str = "/Game/Marvel/Data/DataTable/UI/HeroSkin/UIHeroEmoteTable";
pub const EMOTE_TABLE_MOD_PREFIX: &str = "RivalsSwapper_Emotes_";

/// Rewrite the emote mods for these (default skin, emote id) choices: in the
/// hero's emote table the default emote (`<default>201`) and the chosen one
/// trade keys, so the default emote plays the chosen one in matches, and so do
/// their rows in the lobby emote table. Mods of heroes without a choice are
/// removed. Returns how many heroes' emotes were swapped.
pub fn write_emote_tables(
    game: &Game,
    work_dir: &Path,
    mods_dir: &Path,
    choices: &[(String, String)],
) -> Result<usize> {
    let wanted: HashSet<String> = choices
        .iter()
        .map(|(d, _)| format!("{EMOTE_TABLE_MOD_PREFIX}{}", &d[..4]))
        .collect();
    for entry in std::fs::read_dir(mods_dir).into_iter().flatten().flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        let hero = file
            .strip_prefix(EMOTE_TABLE_MOD_PREFIX)
            .and_then(|f| f.split_once('_').map(|(hero, _)| hero));
        if hero.is_some_and(|hero| !wanted.contains(&format!("{EMOTE_TABLE_MOD_PREFIX}{hero}"))) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let mut swapped = 0;
    for (default, emote) in choices {
        let hero = &default[..4];
        let table = format!("/Game/Marvel/Characters/{hero}/{default}/{default}_EmoteResTable");
        let mod_name = format!("{EMOTE_TABLE_MOD_PREFIX}{hero}");
        let groups = vec![vec![(format!("{default}201"), emote.clone())]];
        swapped += write_table_mod(
            game,
            &work_dir.join(hero),
            mods_dir,
            &mod_name,
            &table,
            groups,
            &|_, _| {},
        )?;
    }
    // The lobby table is keyed by emote id + "0" (its animations' id). Its rows
    // also name the skin an emote belongs to (`302`, `1035302`), and the emote
    // wheel grays out a skin's emote unless that skin is worn: the chosen row
    // takes the default skin's instead (same lengths, patched in place).
    let groups = choices
        .iter()
        .map(|(default, emote)| vec![(format!("{default}2010"), format!("{emote}0"))])
        .collect();
    let unlock = |names: &[String], data: &mut [u8]| {
        for (default, emote) in choices {
            let skin = &emote[..7];
            if skin == default || !skin.starts_with(&default[..4]) {
                continue;
            }
            let Some(row) = table_row(names, data, &format!("{emote}0")) else {
                continue;
            };
            for (from, to) in [(skin, default.as_str()), (&skin[4..], &default[4..])] {
                let (from, to) = (fstring(from), fstring(to));
                if let Some(i) = data[row.clone()]
                    .windows(from.len())
                    .position(|w| w == from.as_slice())
                {
                    data[row.start + i..row.start + i + to.len()].copy_from_slice(&to);
                }
            }
        }
    };
    write_table_mod(
        game,
        work_dir,
        mods_dir,
        EMOTE_UI_TABLE_MOD,
        EMOTE_UI_TABLE,
        groups,
        &unlock,
    )?;
    Ok(swapped)
}

/// Rewrite a shared table mod: the game's `table` with each group's name pairs
/// swapped in its name map, so every reference to one name resolves to the other
/// (rows trade keys or fields). A group applies only if all its names are in the
/// table. `patch` then edits the export data in place (given the table's
/// original names). No group applies: the mod is removed. Returns how many
/// groups applied.
fn write_table_mod(
    game: &Game,
    work_dir: &Path,
    mods_dir: &Path,
    mod_name: &str,
    table: &str,
    groups: Vec<Vec<(String, String)>>,
    patch: &dyn Fn(&[String], &mut [u8]),
) -> Result<usize> {
    for f in mod_files(mods_dir, mod_name) {
        let _ = std::fs::remove_file(f);
    }
    if groups.is_empty() {
        return Ok(0);
    }
    let _ = std::fs::remove_dir_all(work_dir);
    let result = (|| {
        let content_dir = work_dir.join("staging/Marvel/Content");
        let extracted = game.extract_package(table, &work_dir.join("extract"))?;
        let dst = stage(&extracted, &content_dir, table)?;
        let original = names(&dst)?;
        let names: HashSet<String> = original.iter().cloned().collect();
        let mut renames = HashMap::new();
        let mut applied = 0;
        for group in groups {
            if !group
                .iter()
                .all(|(a, b)| names.contains(a) && names.contains(b))
            {
                continue;
            }
            applied += 1;
            for (a, b) in group {
                renames.insert(b.clone(), a.clone());
                renames.insert(a, b);
            }
        }
        if applied == 0 {
            return Ok(0);
        }
        relocate_package(&dst, table, &renames)?;
        let uexp = dst.with_extension("uexp");
        let mut data = std::fs::read(&uexp)?;
        patch(&original, &mut data);
        std::fs::write(&uexp, data)?;
        pack(mods_dir, mod_name, &content_dir)?;
        Ok(applied)
    })();
    let _ = std::fs::remove_dir_all(work_dir);
    result
}

/// Copy an extracted package (.uasset/.uexp/.ubulk) into the staging tree at
/// `target`'s path. Returns the staged .uasset.
fn stage(uasset: &Path, content_dir: &Path, target: &str) -> Result<PathBuf> {
    let dst = content_dir.join(target.trim_start_matches("/Game/"));
    std::fs::create_dir_all(dst.parent().unwrap())?;
    for ext in ["uasset", "uexp", "ubulk"] {
        let src = uasset.with_extension(ext);
        if src.exists() {
            std::fs::copy(&src, dst.with_extension(ext))?;
        }
    }
    Ok(dst.with_extension("uasset"))
}

pub struct Retexture {
    /// Skin whose mesh the retexture is drawn on.
    pub base_skin: String,
    /// Material slot suffix ("Body", "Equip_01", ...) -> the retexture's MI package.
    pub materials: HashMap<String, String>,
    /// The same for the lobby (menu) model's materials (Materials/Lobby/).
    pub lobby_materials: HashMap<String, String>,
}

/// Read a retexture's ChildBP: the mesh skin it inherits from and the material
/// instances it overrides with.
pub fn retexture_source(
    game: &Game,
    work_dir: &Path,
    char_id: &str,
    skin_id: &str,
) -> Result<Retexture> {
    let index = game.skins();
    ensure!(
        index.blueprints.contains(skin_id),
        "This skin has neither its own mesh nor a skin blueprint, so there's nothing to swap."
    );
    let bp = format!("/Game/Marvel/Characters/{char_id}/{skin_id}/{skin_id}_ChildBP");
    let bp_names = names(&game.extract_package(&bp, &work_dir.join(skin_id))?)?;

    let mi_re = regex::Regex::new(&format!(
        r"^/Game/Marvel/Characters/{char_id}/{skin_id}/(?:.*/)?MI_{skin_id}_(.+)$"
    ))?;
    let materials: HashMap<String, String> = bp_names
        .iter()
        .filter(|n| !n.contains("/Lobby/"))
        .filter_map(|n| Some((mi_re.captures(n)?[1].to_string(), n.clone())))
        .collect();

    // A mesh set directly on the blueprint (e.g. .../1018305/Meshes/SK_1018 + FName number).
    // Lobby materials aren't referenced by the ChildBP; they ship in Materials/Lobby.
    let lobby_materials = index
        .lobby_materials
        .get(skin_id)
        .cloned()
        .unwrap_or_default();
    let mesh_re = regex::Regex::new(&format!(
        r"^/Game/Marvel/Characters/{char_id}/(\d{{7}})/Mesh(?:es)?/"
    ))?;
    if let Some(base) = bp_names
        .iter()
        .filter_map(|n| Some(mesh_re.captures(n)?[1].to_string()))
        .find(|id| *id != skin_id && index.meshes.contains_key(id))
    {
        return Ok(Retexture {
            base_skin: base,
            materials,
            lobby_materials,
        });
    }

    // Otherwise the mesh is inherited. Parents and grandparents are all referenced;
    // the closest is the non-default one, and it may be a retexture itself.
    let parent_re = regex::Regex::new(&format!(
        r"^/Game/Marvel/Characters/{char_id}/(\d{{7}})/(\d{{7}})_ChildBP$"
    ))?;
    let mut parents: Vec<&str> = bp_names
        .iter()
        .filter_map(|n| {
            parent_re
                .captures(n)
                .filter(|m| m[1] == m[2])
                .map(|m| m.get(1).unwrap().as_str())
        })
        .filter(|id| *id != skin_id)
        .collect();
    parents.sort_by_key(|id| id.ends_with("001"));
    for parent in parents {
        if index.meshes.contains_key(parent) {
            return Ok(Retexture {
                base_skin: parent.to_string(),
                materials,
                lobby_materials,
            });
        }
        if index.blueprints.contains(parent) && !work_dir.join(parent).exists() {
            let mut inherited = retexture_source(game, work_dir, char_id, parent)?;
            inherited.materials.extend(materials); // this skin's overrides win
            inherited.lobby_materials.extend(lobby_materials);
            return Ok(inherited);
        }
    }
    bail!("Couldn't find the mesh this retexture is based on.")
}

/// Name-map renames that point a mesh's material imports (both the package path
/// and the object name) at a retexture's materials. Slots match by the part
/// after `MI_[Hero_]{skin id}_` (the mesh may borrow another skin's materials), then by
/// that part without its trailing number ("Body_01" -> "Body") when it's
/// unambiguous. Unmatched slots keep the mesh's material, as in-game.
pub fn material_renames(
    mesh_names: &[String],
    materials: &HashMap<String, String>,
    lobby: bool,
) -> HashMap<String, String> {
    let slot_re = regex::Regex::new(r"^MI_(?:[A-Za-z]+_)?\d{7}_(.+)$").unwrap();
    // "Equip_01" == "Equip01"; loosely, both are "equip".
    let norm = |s: &str| s.to_lowercase().replace('_', "");
    let loose = |s: &str| {
        norm(s)
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .to_string()
    };
    // (package path, object name, slot)
    let slots: Vec<(&String, &str, String)> = mesh_names
        .iter()
        .filter(|n| n.starts_with('/') && n.contains("/Lobby/") == lobby)
        .filter_map(|n| {
            let obj = n.rsplit_once('/')?.1;
            Some((n, obj, slot_re.captures(obj)?[1].to_string()))
        })
        .collect();
    let exact_key = |slot: &str| materials.keys().find(|k| norm(k) == norm(slot));
    // Materials some slot matches exactly aren't offered to the others.
    let exact: HashSet<&str> = slots
        .iter()
        .filter_map(|s| exact_key(&s.2))
        .map(|k| k.as_str())
        .collect();
    let mut renames = HashMap::new();
    for (path, obj, slot) in &slots {
        let target = if let Some(k) = exact_key(slot) {
            Some(k.as_str())
        } else {
            let mut c = materials
                .keys()
                .filter(|k| !exact.contains(k.as_str()) && loose(k) == loose(slot));
            match (c.next(), c.next()) {
                (Some(k), None) => Some(k.as_str()),
                // Last resort for typos in the game's own names ("Boby" for "Body").
                (None, _) => {
                    let mut c = materials.keys().filter(|k| {
                        !exact.contains(k.as_str())
                            && loose(slot).len() >= 4
                            && one_edit_apart(&loose(k), &loose(slot))
                    });
                    match (c.next(), c.next()) {
                        (Some(k), None) => Some(k.as_str()),
                        _ => None,
                    }
                }
                _ => None,
            }
        };
        if let Some(target) = target {
            let new_path = &materials[target];
            renames.insert((*path).clone(), new_path.clone());
            renames.insert(
                obj.to_string(),
                new_path.rsplit('/').next().unwrap_or(new_path).to_string(),
            );
        }
    }
    renames
}

/// Same length with one substitution, or one insertion/deletion.
fn one_edit_apart(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    if long.len() - short.len() > 1 || a == b {
        return false;
    }
    let i = short
        .iter()
        .zip(long.iter())
        .take_while(|(x, y)| x == y)
        .count();
    let skip = if short.len() == long.len() { 1 } else { 0 };
    short[i + skip..] == long[i + 1..]
}

// ---------------------------------------------------------------------------
// relocate_package: move a cooked package to a new path of any length.
// Rewrites FolderName and the package's own name-map entry, then shifts every
// absolute offset in the header and export table. The .uexp/.ubulk need no
// changes (bulk offsets are relative to the .ubulk / BulkDataStartOffset).
// Layout is Marvel Rivals' cooked UE5.3 package (LegacyFileVersion -8).
// ---------------------------------------------------------------------------

const OFFSET_FIELDS: [&str; 13] = [
    "NameOffset",
    "SoftObjectPathsOffset",
    "GatherableOffset",
    "ExportOffset",
    "ImportOffset",
    "DependsOffset",
    "SoftPackageRefOffset",
    "SearchableNamesOffset",
    "ThumbnailTableOffset",
    "AssetRegistryDataOffset",
    "WorldTileInfoOffset",
    "PreloadDepOffset",
    "DataResourceOffset",
];

fn i32_at(d: &[u8], o: usize) -> Result<i32> {
    Ok(i32::from_le_bytes(
        d.get(o..o + 4).context("package truncated")?.try_into()?,
    ))
}
fn i64_at(d: &[u8], o: usize) -> Result<i64> {
    Ok(i64::from_le_bytes(
        d.get(o..o + 8).context("package truncated")?.try_into()?,
    ))
}
fn fstring_size(d: &[u8], o: usize) -> Result<usize> {
    let n = i32_at(d, o)?;
    Ok(4 + if n >= 0 {
        n as usize
    } else {
        n.unsigned_abs() as usize * 2
    })
}
pub(crate) fn fstring(s: &str) -> Vec<u8> {
    let mut v = ((s.len() + 1) as i32).to_le_bytes().to_vec();
    v.extend(s.as_bytes());
    v.push(0);
    v
}

/// Byte positions of the summary fields (not their values).
fn summary_positions(d: &[u8]) -> Result<HashMap<&'static str, usize>> {
    ensure!(i32_at(d, 4)? == -8, "unexpected package version");
    let mut pos = HashMap::new();
    let mut o = 24;
    o += 4 + usize::try_from(i32_at(d, o)?)? * 20; // custom versions
    pos.insert("TotalHeaderSize", o);
    o += 4;
    pos.insert("FolderName", o);
    o += fstring_size(d, o)?;
    o += 4; // PackageFlags
    for name in [
        "NameCount",
        "NameOffset",
        "SoftObjectPathsCount",
        "SoftObjectPathsOffset",
        "GatherableCount",
        "GatherableOffset",
        "ExportCount",
        "ExportOffset",
        "ImportCount",
        "ImportOffset",
        "DependsOffset",
        "SoftPackageRefCount",
        "SoftPackageRefOffset",
        "SearchableNamesOffset",
        "ThumbnailTableOffset",
    ] {
        pos.insert(name, o);
        o += 4;
    }
    o += 16; // Guid
    o += 4 + usize::try_from(i32_at(d, o)?)? * 8; // Generations
    for _ in 0..2 {
        // SavedByEngineVersion, CompatibleWithEngineVersion
        o += 10;
        o += fstring_size(d, o)?;
    }
    o += 16; // CompressionFlags, CompressedChunks, PackageSource, AdditionalPackagesToCook
    pos.insert("AssetRegistryDataOffset", o);
    o += 4;
    pos.insert("BulkDataStartOffset", o);
    o += 8;
    pos.insert("WorldTileInfoOffset", o);
    o += 4;
    o += 4 + usize::try_from(i32_at(d, o)?)? * 4; // ChunkIDs
    o += 4; // PreloadDependencyCount
    pos.insert("PreloadDepOffset", o);
    o += 4;
    o += 4 + 8; // NamesReferencedFromExportDataCount, PayloadTocOffset
    pos.insert("DataResourceOffset", o);
    o += 4;
    ensure!(
        o as i64 == i32_at(d, pos["NameOffset"])? as i64,
        "Unexpected package summary layout"
    );
    Ok(pos)
}

/// Name-map entries as (name, start, end); `end` is past the 4-byte hash.
fn name_entries(raw: &[u8], pos: &HashMap<&str, usize>) -> Result<Vec<(String, usize, usize)>> {
    let mut p = i32_at(raw, pos["NameOffset"])? as usize;
    let mut out = vec![];
    for _ in 0..i32_at(raw, pos["NameCount"])? {
        let len = i32_at(raw, p)?;
        let end = p + fstring_size(raw, p)? + 4;
        let name = if len > 0 {
            String::from_utf8_lossy(
                raw.get(p + 4..p + 3 + len as usize)
                    .context("package truncated")?,
            )
            .into_owned()
        } else {
            String::new() // UTF-16 names never hold the ASCII paths we rewrite
        };
        out.push((name, p, end));
        p = end;
    }
    Ok(out)
}

/// The package's name map.
pub fn names(uasset: &Path) -> Result<Vec<String>> {
    let raw = std::fs::read(uasset)?;
    Ok(name_entries(&raw, &summary_positions(&raw)?)?
        .into_iter()
        .map(|(n, ..)| n)
        .collect())
}

/// "SK_1048_1048309" -> ("SK_1048", 1048310): UE's FName split. Numbers with a
/// leading zero aren't split.
fn split_fname(name: &str) -> (&str, i32) {
    match name.rsplit_once('_') {
        Some((head, n))
            if !n.is_empty()
                && n.bytes().all(|b| b.is_ascii_digit())
                && !(n.len() > 1 && n.starts_with('0')) =>
        {
            match n.parse::<i32>() {
                Ok(n) => (head, n + 1),
                Err(_) => (name, 0),
            }
        }
        _ => (name, 0),
    }
}

/// Returns the old package path. `renames` rewrites other name-map entries
/// (e.g. material import paths) in the same pass.
pub fn relocate_package(
    uasset: &Path,
    new_package_path: &str,
    renames: &HashMap<String, String>,
) -> Result<String> {
    let raw = std::fs::read(uasset)?;
    let pos = summary_positions(&raw)?;
    let read = |f: &str| i32_at(&raw, pos[f]);
    let name_offset = read("NameOffset")? as usize;
    let export_count = read("ExportCount")? as usize;
    let export_offset = read("ExportOffset")? as i64;

    let folder = pos["FolderName"];
    let folder_len = read("FolderName")?;
    ensure!(folder_len > 0, "FolderName missing or not ASCII");
    let old_folder_end = folder + 4 + folder_len as usize;
    let old_path = String::from_utf8(raw[folder + 4..old_folder_end - 1].to_vec())?;
    let (old_dir, old_base) = old_path.rsplit_once('/').context("bad FolderName")?;
    let (new_dir, new_base) = new_package_path
        .rsplit_once('/')
        .context("bad package path")?;

    // Names like SK_1048_1048309 are stored as "SK_1048" plus FName number
    // 1048310; the package path likewise as ".../SK_1048" plus that number.
    let (old_head, old_num) = split_fname(old_base);
    let (new_head, new_num) = split_fname(new_base);
    let mut renames = renames.clone();
    renames.insert(old_path.clone(), new_package_path.to_string());
    renames.insert(
        format!("{old_dir}/{old_head}"),
        format!("{new_dir}/{new_head}"),
    );
    if old_head != new_head {
        renames.insert(old_head.to_string(), new_head.to_string()); // the main object's name
    }

    let mut new_map = vec![];
    let entries = name_entries(&raw, &pos)?;
    let head_index = entries.iter().position(|(n, ..)| n == old_head);
    for (name, start, end) in &entries {
        let new_name = renames.get(name).cloned();
        match new_name {
            Some(n) => {
                new_map.extend(fstring(&n));
                new_map.extend(&raw[end - 4..*end]); // keep original hash
            }
            None => new_map.extend(&raw[*start..*end]),
        }
    }
    let names_end = entries.last().map_or(name_offset, |e| e.2);

    let new_folder = fstring(new_package_path);
    let d_folder = new_folder.len() as i64 - (old_folder_end - folder) as i64;
    let d_total = d_folder + new_map.len() as i64 - (names_end - name_offset) as i64;

    let mut buf = raw[..folder].to_vec();
    buf.extend(&new_folder);
    buf.extend(&raw[old_folder_end..name_offset]);
    buf.extend(&new_map);
    buf.extend(&raw[names_end..]);

    let shifted = |v: i64| match v {
        v if v <= 0 => v,
        v if v >= names_end as i64 => v + d_total,
        v if v >= name_offset as i64 => v + d_folder,
        v => v,
    };
    let at = |p: usize| {
        if p < folder {
            p
        } else {
            (p as i64 + d_folder) as usize
        }
    };
    let put_i32 = |buf: &mut Vec<u8>, p: usize, v: i64| {
        buf[p..p + 4].copy_from_slice(&(v as i32).to_le_bytes())
    };
    let put_i64 =
        |buf: &mut Vec<u8>, p: usize, v: i64| buf[p..p + 8].copy_from_slice(&v.to_le_bytes());

    for field in OFFSET_FIELDS {
        put_i32(&mut buf, at(pos[field]), shifted(read(field)? as i64));
    }
    put_i32(
        &mut buf,
        pos["TotalHeaderSize"],
        read("TotalHeaderSize")? as i64 + d_total,
    );
    put_i64(
        &mut buf,
        at(pos["BulkDataStartOffset"]),
        i64_at(&raw, pos["BulkDataStartOffset"])? + d_total,
    );

    // Export table entries are 96 bytes; ObjectName (name index, number) sits at
    // +0x10 and SerialOffset (int64) at +0x24. Renaming the main object to the
    // new package's name only changes the number (or the head entry above).
    let new_export_offset = shifted(export_offset) as usize;
    for i in 0..export_count {
        let e = new_export_offset + i * 96;
        let v = i64_at(&buf, e + 0x24)?;
        put_i64(&mut buf, e + 0x24, v + d_total);
        if Some(i32_at(&buf, e + 0x10)? as usize) == head_index
            && i32_at(&buf, e + 0x14)? == old_num
        {
            put_i32(&mut buf, e + 0x14, new_num as i64);
        }
    }

    std::fs::write(uasset, buf)?;
    Ok(old_path)
}

/// Give script imports with a numbered name (NetEase's Python classes, like
/// `PyAbility_103521.PyCue_HitImpact_10352101`, stored as `PyCue_HitImpact` +
/// number) an explicit name of their own. UAssetTool drops FName numbers when it
/// hashes script imports, which points them at classes that don't exist and
/// crashes the game while loading; with the number spelled out the hash is right.
/// Returns how many names were spelled out.
pub fn spell_out_script_import_numbers(uasset: &Path) -> Result<usize> {
    let raw = std::fs::read(uasset)?;
    let pos = summary_positions(&raw)?;
    let read = |f: &str| i32_at(&raw, pos[f]);
    let entries = name_entries(&raw, &pos)?;
    let names: Vec<&str> = entries.iter().map(|e| e.0.as_str()).collect();
    let (import_offset, import_count) = (
        read("ImportOffset")? as usize,
        read("ImportCount")? as usize,
    );
    // Import entries are 32 bytes: ClassPackage, ClassName (FNames), OuterIndex,
    // ObjectName (FName), bImportOptional.
    let import = |i: usize| import_offset + i * 32;
    let root_name = |mut i: usize| -> Result<usize> {
        for _ in 0..import_count {
            let outer = i32_at(&raw, import(i) + 16)?;
            if outer >= 0 {
                break;
            }
            i = (-outer - 1) as usize;
        }
        Ok(i32_at(&raw, import(i) + 20)? as usize)
    };
    let mut added: Vec<String> = vec![];
    let mut patches = vec![]; // (byte offset of an FName, index into names + added)
    for i in 0..import_count {
        if !names
            .get(root_name(i)?)
            .is_some_and(|n| n.starts_with("/Script/"))
        {
            continue;
        }
        for field in [0, 8, 20] {
            let at = import(i) + field;
            let (index, number) = (i32_at(&raw, at)? as usize, i32_at(&raw, at + 4)?);
            if number == 0 {
                continue;
            }
            let full = format!("{}_{}", names[index], number - 1);
            let slot = match added.iter().position(|n| *n == full) {
                Some(k) => names.len() + k,
                None => {
                    added.push(full);
                    names.len() + added.len() - 1
                }
            };
            patches.push((at, slot));
        }
    }
    if added.is_empty() {
        return Ok(0);
    }

    // Append the names after the name map and shift everything behind it.
    let names_end = entries.last().map_or(read("NameOffset")? as usize, |e| e.2);
    let mut new_names = vec![];
    for n in &added {
        new_names.extend(fstring(n));
        new_names.extend([0u8; 4]); // hash; the packer recomputes name hashes
    }
    let delta = new_names.len() as i64;
    let mut buf = raw[..names_end].to_vec();
    buf.extend(&new_names);
    buf.extend(&raw[names_end..]);
    let put_i32 = |buf: &mut Vec<u8>, p: usize, v: i64| {
        buf[p..p + 4].copy_from_slice(&(v as i32).to_le_bytes())
    };
    let put_i64 =
        |buf: &mut Vec<u8>, p: usize, v: i64| buf[p..p + 8].copy_from_slice(&v.to_le_bytes());
    let shifted = |v: i64| if v >= names_end as i64 { v + delta } else { v };
    for field in OFFSET_FIELDS {
        put_i32(&mut buf, pos[field], shifted(read(field)? as i64));
    }
    put_i32(
        &mut buf,
        pos["NameCount"],
        read("NameCount")? as i64 + added.len() as i64,
    );
    put_i32(
        &mut buf,
        pos["TotalHeaderSize"],
        read("TotalHeaderSize")? as i64 + delta,
    );
    put_i64(
        &mut buf,
        pos["BulkDataStartOffset"],
        i64_at(&raw, pos["BulkDataStartOffset"])? + delta,
    );
    let export_offset = shifted(read("ExportOffset")? as i64) as usize;
    for i in 0..read("ExportCount")? as usize {
        let e = export_offset + i * 96 + 0x24; // SerialOffset
        let v = i64_at(&buf, e)?;
        put_i64(&mut buf, e, v + delta);
    }
    for (at, slot) in patches {
        let at = shifted(at as i64) as usize;
        put_i32(&mut buf, at, slot as i64);
        put_i32(&mut buf, at + 4, 0);
    }
    std::fs::write(uasset, buf)?;
    Ok(added.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Moves a real mesh onto another skin's path and name: the FolderName and
    /// the top-level object name must both follow. Run: cargo test relocate -- --ignored
    #[test]
    #[ignore]
    fn relocate_roundtrip() {
        let paks = PathBuf::from(crate::settings::Settings::load().game_paks_dir);
        let game = Game::open(&paks).unwrap();
        let tmp = std::env::temp_dir().join("rivals_swapper_relocate");
        let _ = std::fs::remove_dir_all(&tmp);
        let mesh = game
            .extract_package(
                "/Game/Marvel/Characters/1048/1048301/Meshes/SK_1048_1048301",
                &tmp.join("x"),
            )
            .unwrap();
        let target = "/Game/Marvel/Characters/1048/1048001/Meshes/SK_1048_1048001";
        let old = relocate_package(&mesh, target, &HashMap::new()).unwrap();
        assert_eq!(
            old,
            "/Game/Marvel/Characters/1048/1048301/Meshes/SK_1048_1048301"
        );
        assert_eq!(
            package_identity(&mesh).unwrap(),
            (target.to_string(), "SK_1048_1048001".to_string())
        );
    }

    #[test]
    fn material_slots_match() {
        let base = "/Game/Marvel/Characters/1046/1046001/Materials";
        let retex = "/Game/Marvel/Characters/1046/1046101/Materials";
        let cape = "/Game/Marvel/Characters/1046/1046300/Materials/MI_1046300_Cape_01";
        let mesh_names: Vec<String> = vec![
            format!("{base}/MI_1046001_Boby"),
            "MI_1046001_Boby".into(),
            format!("{base}/MI_1046001_Hair_01"),
            "MI_1046001_Hair_01".into(),
            format!("{base}/MI_1046001_Eyes_01"),
            format!("{base}/MI_1046001_Eyes_02"),
            format!("{base}/MI_1046001_Tooth"),
            format!("{base}/MI_Hero_1046001_Equip_01"),
            format!("{base}/Lobby/MI_1046001_Head"),
            cape.to_string(),
        ];
        let materials: HashMap<String, String> = [
            "Body", "Hair_01", "Eyes", "Cape", "Tooth_01", "Tooth_02", "Equip01",
        ]
        .map(|s| (s.to_string(), format!("{retex}/MI_1046101_{s}")))
        .into();
        let r = material_renames(&mesh_names, &materials, false);
        assert_eq!(
            r[&format!("{base}/MI_1046001_Boby")],
            format!("{retex}/MI_1046101_Body")
        ); // typo
        assert_eq!(r["MI_1046001_Boby"], "MI_1046101_Body"); // object name follows
        assert_eq!(
            r[&format!("{base}/MI_1046001_Hair_01")],
            format!("{retex}/MI_1046101_Hair_01")
        ); // exact
        assert_eq!(r[cape], format!("{retex}/MI_1046101_Cape")); // other skin's MI, number stripped
        // Both eye slots take the one Eyes material; Tooth has two candidates, so it's left alone.
        assert_eq!(
            r[&format!("{base}/MI_1046001_Eyes_02")],
            format!("{retex}/MI_1046101_Eyes")
        );
        assert!(!r.contains_key(&format!("{base}/MI_1046001_Tooth")));
        assert!(!r.keys().any(|k| k.contains("/Lobby/")));
        // Hero-name infix on the mesh side, no underscore on the retexture side.
        assert_eq!(
            r[&format!("{base}/MI_Hero_1046001_Equip_01")],
            format!("{retex}/MI_1046101_Equip01")
        );
        assert!(
            one_edit_apart("boby", "body")
                && one_edit_apart("eye", "eyes")
                && !one_edit_apart("hair", "hand")
        );
    }

    #[test]
    fn fname_split() {
        assert_eq!(split_fname("SK_1048_1048301"), ("SK_1048", 1048302));
        assert_eq!(split_fname("MI_1048001_Body_01"), ("MI_1048001_Body_01", 0)); // leading zero isn't split
        assert_eq!(split_fname("GAL_SkinConfig"), ("GAL_SkinConfig", 0));
    }

    fn installed_game() -> (PathBuf, Game, Vec<crate::db::Character>) {
        let paks = PathBuf::from(crate::settings::Settings::load().game_paks_dir);
        let game = Game::open(&paks).unwrap();
        let locres = game
            .read_pak_file("Marvel/Content/Localization/Game/en/Game.locres")
            .unwrap();
        let chars = crate::db::build(&crate::db::parse_locres(&locres).unwrap(), game.skins());
        (paks, game, chars)
    }

    /// Every recolor in the installed game: base mesh found and materials
    /// matched. Run: cargo test survey -- --ignored --nocapture
    #[test]
    #[ignore]
    fn survey_retextures() {
        let (_, game, chars) = installed_game();
        let work = std::env::temp_dir().join("rivals_swapper_survey");
        let mut mesh_names: HashMap<String, Vec<String>> = HashMap::new();
        let (mut full, mut part, mut fail) = (0, 0, 0);
        for c in &chars {
            for s in c.alt_skins().filter(|s| s.retexture) {
                let _ = std::fs::remove_dir_all(&work);
                let res = retexture_source(&game, &work, &c.char_id, &s.id).and_then(|r| {
                    let mut matched = HashSet::new();
                    for (_, pkg) in &game.skins().meshes[&r.base_skin] {
                        if !mesh_names.contains_key(pkg) {
                            mesh_names.insert(
                                pkg.clone(),
                                names(&game.extract_package(pkg, &work.join("m"))?)?,
                            );
                        }
                        matched.extend(
                            material_renames(&mesh_names[pkg], &r.materials, false)
                                .into_values()
                                .filter(|v| v.starts_with('/')),
                        );
                    }
                    let missing: Vec<_> = r
                        .materials
                        .iter()
                        .filter(|(_, p)| !matched.contains(*p))
                        .map(|(k, _)| k.clone())
                        .collect();
                    Ok((r.base_skin, matched.len(), r.materials.len(), missing))
                });
                match res {
                    Ok((base, m, n, missing)) => {
                        if m == n {
                            full += 1
                        } else {
                            part += 1
                        }
                        println!(
                            "OK {} {} ({}) base {base}: {m}/{n} materials, unmatched {missing:?}",
                            c.name, s.id, s.name
                        );
                    }
                    Err(e) => {
                        fail += 1;
                        println!("FAIL {} {} ({}): {e:#}", c.name, s.id, s.name);
                    }
                }
            }
        }
        println!("SUMMARY full={full} partial={part} fail={fail}");
    }

    /// Full swaps against the installed game into a temp dir: a mesh skin, a
    /// recolor, and a multi-form hero. Each packed mod is reopened over the game:
    /// its mesh must win at the default skin's path, under the vanilla name.
    /// Run: cargo test real_swap -- --ignored --nocapture
    /// A cue whose parent is a numbered Python class (PyCue_HitImpact_10352101):
    /// packed as-is UAssetTool hashes the class wrongly; with the number spelled
    /// out every script import matches the game's own package.
    #[test]
    #[ignore]
    fn python_class_imports_survive_packing() {
        let paks = PathBuf::from(crate::settings::Settings::load().game_paks_dir);
        let game = Game::open(&paks).unwrap();
        let src =
            "/Game/Marvel/Characters/1035/1035302/Cues/103521/Cue_Scope_HitImpact_10352101_BP";
        let root = std::env::temp_dir().join("rivals_swapper_pyclass");
        let _ = std::fs::remove_dir_all(&root);
        for spell in [false, true] {
            let dir = root.join(spell.to_string());
            let content = dir.join("staging/Marvel/Content");
            let staged = stage(
                &game.extract_package(src, &dir.join("x")).unwrap(),
                &content,
                src,
            )
            .unwrap();
            if spell {
                assert_eq!(spell_out_script_import_numbers(&staged).unwrap(), 3);
            }
            command(crate::uassettool())
                .arg("create_mod_iostore")
                .arg(dir.join("t_9999999_P"))
                .arg(&content)
                .output()
                .unwrap();
            let utoc = dir.join("t_9999999_P.utoc");
            assert_eq!(game.script_imports_survive(src, &utoc, src).unwrap(), spell);
        }
    }

    /// The skin table points Venom's default row at Gummy Surprise's ChildBP:
    /// the two ChildBP names trade places in the packed table's name map.
    #[test]
    #[ignore]
    #[cfg(unix)]
    fn skin_table_swaps_childbp() {
        let paks = PathBuf::from(crate::settings::Settings::load().game_paks_dir);
        let game = Game::open(&paks).unwrap();
        let root = std::env::temp_dir().join("rivals_swapper_table");
        let _ = std::fs::remove_dir_all(&root);
        let pairs = [("1035001".to_string(), "1035302".to_string())];
        assert_eq!(
            write_skin_table(&game, &root.join("work"), &root, &pairs).unwrap(),
            1
        );
        let utoc = &mod_files(&root, SKIN_TABLE_MOD)[1];
        assert!(
            game.script_imports_survive(SKIN_TABLE, utoc, SKIN_TABLE)
                .unwrap()
        );
        let view = root.join("view");
        std::fs::create_dir_all(&view).unwrap();
        for f in std::fs::read_dir(&paks)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .chain(mod_files(&root, SKIN_TABLE_MOD))
        {
            std::os::unix::fs::symlink(&f, view.join(f.file_name().unwrap())).unwrap();
        }
        let index = |g: &Game, dir: &str, n: &str| {
            names(&g.extract_package(SKIN_TABLE, &root.join(dir)).unwrap())
                .unwrap()
                .iter()
                .position(|x| x == n)
                .unwrap()
        };
        let packed = Game::open(&view).unwrap();
        let (d, s) = (
            "/Game/Marvel/Characters/1035/1035001/1035001_ChildBP",
            "/Game/Marvel/Characters/1035/1035302/1035302_ChildBP",
        );
        assert_eq!(index(&game, "a", d), index(&packed, "b", s));
        assert_eq!(index(&game, "c", s), index(&packed, "d", d));
        // No pairs: the mod is gone.
        write_skin_table(&game, &root.join("work"), &root, &[]).unwrap();
        assert!(mod_files(&root, SKIN_TABLE_MOD).iter().all(|f| !f.exists()));
    }

    /// MVP swaps in the shared MVP table: Venom's default MVP id takes Gummy
    /// Surprise's special MVP row, and (separately) the default row plays Gummy
    /// Surprise's version of the default MVP.
    #[test]
    #[ignore]
    #[cfg(unix)]
    fn mvp_table_swaps() {
        let paks = PathBuf::from(crate::settings::Settings::load().game_paks_dir);
        let game = Game::open(&paks).unwrap();
        let root = std::env::temp_dir().join("rivals_swapper_mvp");
        let _ = std::fs::remove_dir_all(&root);
        let mvps = crate::db::mvps(&game, &root.join("list")).unwrap();
        let sequence = |id: &str| mvps.iter().find(|m| m.id == id).unwrap().package.clone();
        let index = |g: &Game, dir: &str, n: &str| {
            names(&g.extract_package(MVP_TABLE, &root.join(dir)).unwrap())
                .unwrap()
                .iter()
                .position(|x| x == n)
                .unwrap()
        };
        let cases = [
            (
                MvpChoice::Special {
                    default_id: "1035001301".into(),
                    mvp_id: "1035302306".into(),
                    default_skin: "1035001".into(),
                    skins: vec!["1035304".into(), "1035302".into()],
                },
                "1035001301".to_string(),
                "1035302306".to_string(),
            ),
            (
                MvpChoice::SkinVersion {
                    default_sequence: sequence("1035001301"),
                    skin_sequence: sequence("1035302301"),
                },
                sequence("1035001301"),
                sequence("1035302301"),
            ),
        ];
        for (i, (choice, a, b)) in cases.into_iter().enumerate() {
            let out = root.join(i.to_string());
            assert_eq!(
                write_mvp_table(&game, &out.join("work"), &out, &[choice]).unwrap(),
                1
            );
            let utoc = &mod_files(&out, MVP_TABLE_MOD)[1];
            assert!(
                game.script_imports_survive(MVP_TABLE, utoc, MVP_TABLE)
                    .unwrap()
            );
            let view = out.join("view");
            std::fs::create_dir_all(&view).unwrap();
            for f in std::fs::read_dir(&paks)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .chain(mod_files(&out, MVP_TABLE_MOD))
            {
                std::os::unix::fs::symlink(&f, view.join(f.file_name().unwrap())).unwrap();
            }
            let packed = Game::open(&view).unwrap();
            assert_eq!(
                index(&game, "o1", &a),
                index(&packed, &format!("p{i}a"), &b)
            );
            assert_eq!(
                index(&game, "o2", &b),
                index(&packed, &format!("p{i}b"), &a)
            );
            if i == 0 {
                // The row the default MVP id now names is the special one, with an
                // entry for the default skin.
                let table = packed
                    .extract_package(MVP_TABLE, &root.join("row"))
                    .unwrap();
                let (names, data) = (
                    names(&table).unwrap(),
                    std::fs::read(table.with_extension("uexp")).unwrap(),
                );
                let row = &data[table_row(&names, &data, "1035001301").unwrap()];
                let has = |id: &str| {
                    row.windows(fstring(id).len())
                        .any(|w| w == fstring(id).as_slice())
                };
                assert!(
                    row.len() < 1000,
                    "the default row is far longer: {}",
                    row.len()
                );
                assert!(has("1035001") && !has("1035302"));
            }
        }
    }

    /// Unticking sound effects keeps the default skin's sounds.
    #[test]
    #[ignore]
    fn swap_without_sounds() {
        let (_, game, chars) = installed_game();
        let c = chars.iter().find(|c| c.char_id == "1048").unwrap();
        let skin = c.skins.iter().find(|s| s.id == "1048304").unwrap();
        let out = std::env::temp_dir().join("rivals_swapper_nosound");
        let _ = std::fs::remove_dir_all(&out);
        for sounds in [true, false] {
            let lines = std::cell::RefCell::new(vec![]);
            create(
                &game,
                &out.join("work"),
                &out,
                c,
                skin,
                SwapOptions {
                    sounds,
                    effects: false,
                    mvp: false,
                },
                &|m| lines.borrow_mut().push(m),
            )
            .unwrap();
            assert_eq!(lines.borrow().iter().any(|l| l.contains("Sounds:")), sounds);
        }
    }

    #[test]
    #[ignore]
    #[cfg(unix)]
    fn real_swap() {
        let (paks, game, chars) = installed_game();
        let root = std::env::temp_dir().join("rivals_swapper_swap");
        let _ = std::fs::remove_dir_all(&root);
        for (hero, skin_id, forms) in [
            ("1048", "1048301", 1),
            ("1048", "1048100", 1),
            ("1048", "1048304", 1),
            ("1035", "1035302", 1),
            ("1011", "1011300", 3),
        ] {
            let c = chars.iter().find(|c| c.char_id == hero).unwrap();
            let skin = c.skins.iter().find(|s| s.id == skin_id).unwrap();
            let out = root.join(skin_id);
            let t = std::time::Instant::now();
            let rec = create(
                &game,
                &out.join("work"),
                &out,
                c,
                skin,
                SwapOptions {
                    sounds: true,
                    effects: true,
                    mvp: true,
                },
                &|m| println!("{m}"),
            )
            .unwrap();
            println!("{} {} took {:?}", c.name, skin.name, t.elapsed());

            // Reopen the packed mod next to links to the game's containers.
            let view = out.join("view");
            std::fs::create_dir_all(&view).unwrap();
            for f in std::fs::read_dir(&paks)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .chain(mod_files(&out, &rec.mod_name))
            {
                if f.exists() {
                    std::os::unix::fs::symlink(&f, view.join(f.file_name().unwrap())).unwrap();
                }
            }
            let packed = Game::open(&view).unwrap();
            assert_eq!(rec.meshes.len(), forms, "{:?}", rec.meshes);
            for (form, target) in &rec.meshes {
                assert!(
                    target.starts_with(&format!(
                        "/Game/Marvel/Characters/{hero}/{}/",
                        c.default_skin_id
                    )),
                    "{target}"
                );
                let file = packed
                    .extract_package(target, &out.join(format!("check{form}")))
                    .unwrap();
                // retoc's re-extraction writes a short FolderName; the object name must match.
                assert_eq!(
                    package_identity(&file).unwrap().1,
                    target.rsplit('/').next().unwrap()
                );
                let names = names(&file).unwrap();
                let own = names
                    .iter()
                    .filter(|n| n.contains(&format!("/{skin_id}/")) && n.contains("/MI_"))
                    .count();
                println!(
                    "  form {form}: {target} now uses {own} of {}'s materials",
                    skin.name
                );
                assert!(own > 0, "{names:?}");
            }
            // Sound tables: a skin with its own sound events plays them from the
            // default skin's table path.
            if game.skins().sound_tables.contains_key(skin_id) {
                let target = &game.skins().sound_tables[&c.default_skin_id]["SoundTable"];
                let file = packed.extract_package(target, &out.join("sound")).unwrap();
                let own = names(&file)
                    .unwrap()
                    .iter()
                    .filter(|n| n.contains(&format!("/WwiseEvents/Characters/{hero}/{skin_id}/")))
                    .count();
                println!(
                    "  sounds: {target} now plays {own} of {}'s own events",
                    skin.name
                );
                assert!(
                    own > 0 || skin_id != "1048304",
                    "Psychedelic Pulse has its own sounds"
                );
                assert_eq!(
                    package_identity(&file).unwrap().1,
                    target.rsplit('/').next().unwrap()
                );
            }
            // Effects: each swapped cue reads back at the default path with every
            // script import (Python classes included) exactly as in the game's own.
            let cues = swappable_cues(&game, &out.join("cues"), skin_id, &c.default_skin_id);
            for (src, target) in &cues {
                let file = packed.extract_package(target, &out.join("cue")).unwrap();
                assert!(
                    names(&file)
                        .unwrap()
                        .iter()
                        .any(|n| n.contains(&format!("/{skin_id}/"))),
                    "{target}"
                );
                assert!(
                    game.script_imports_survive(src, Path::new(&rec.utoc_path), target)
                        .unwrap(),
                    "{target}"
                );
            }
            println!("  effects: {} cues", cues.len());
            if skin_id == "1035302" {
                assert!(!cues.is_empty());
                // An ability montage's notify override, packed into the mod.
                let montage = "/Game/Marvel/Characters/1035/1035001/Animations/103511_Attack01_MT";
                assert!(
                    game.script_imports_survive(montage, Path::new(&rec.utoc_path), montage)
                        .unwrap()
                );
            }
            // Lobby models: every form the default skin has a lobby model for.
            let lobby = &game.skins().lobby[&c.default_skin_id];
            for (form, target) in lobby {
                let file = packed
                    .extract_package(target, &out.join(format!("lobby{form}")))
                    .unwrap();
                assert_eq!(
                    package_identity(&file).unwrap().1,
                    target.rsplit('/').next().unwrap()
                );
                let names = names(&file).unwrap();
                let own = names
                    .iter()
                    .filter(|n| {
                        n.contains(&format!("/{skin_id}/Materials/")) && n.contains("/Lobby/")
                    })
                    .count();
                println!(
                    "  lobby form {form}: {target} now uses {own} of {}'s lobby materials",
                    skin.name
                );
                assert!(own > 0, "{names:?}");
            }
            assert!(
                mod_files(&out, &rec.mod_name)
                    .iter()
                    .skip(1)
                    .all(|f| f.exists())
            );
        }
    }

    /// (FolderName, name of the top-level export) of a legacy .uasset.
    fn package_identity(uasset: &Path) -> Result<(String, String)> {
        let raw = std::fs::read(uasset)?;
        let pos = summary_positions(&raw)?;
        let folder = &raw[pos["FolderName"] + 4..];
        let folder =
            String::from_utf8(folder[..folder.iter().position(|b| *b == 0).unwrap()].to_vec())?;
        let names = name_entries(&raw, &pos)?;
        // The asset itself is the top-level export (OuterIndex 0, at +0x0C).
        let first = i32_at(&raw, pos["ExportOffset"])? as usize;
        let count = i32_at(&raw, pos["ExportCount"])? as usize;
        let e = (0..count)
            .map(|i| first + i * 96)
            .find(|e| i32_at(&raw, e + 0x0C).is_ok_and(|o| o == 0))
            .context("no top-level export")?;
        let (idx, num) = (i32_at(&raw, e + 0x10)? as usize, i32_at(&raw, e + 0x14)?);
        let base = &names[idx].0;
        Ok((
            folder,
            if num > 0 {
                format!("{base}_{}", num - 1)
            } else {
                base.clone()
            },
        ))
    }
}

/// Performance budget (see PERF.md): the database build and a whole swap job
/// (open the game, swap a skin with every extra, rewrite the table mods) stay
/// well under these on the installed game. Roughly 2.4 s and 4.7 s on the
/// machine they were measured on; budgets are about twice that.
#[cfg(test)]
#[test]
#[ignore]
fn perf_budget() {
    use std::time::{Duration, Instant};
    let paks = PathBuf::from(crate::settings::Settings::load().game_paks_dir);
    let root = PathBuf::from("target/perf_budget");
    let _ = std::fs::remove_dir_all(&root);
    let game = Game::open(&paks).unwrap();
    let t = Instant::now();
    let chars = crate::db::build_and_cache(&game, &root.join("db.json"), "perf").unwrap();
    let build = t.elapsed();
    drop(game);
    let c = chars.iter().find(|c| c.char_id == "1035").unwrap();
    let skin = c.skins.iter().find(|s| s.id == "1035302").unwrap();
    let t = Instant::now();
    let game = Game::open(&paks).unwrap();
    let options = SwapOptions {
        sounds: true,
        effects: true,
        mvp: true,
    };
    create(&game, &root.join("work"), &root, c, skin, options, &|_| {}).unwrap();
    let tables = root.join("tables");
    write_skin_table(
        &game,
        &tables,
        &root,
        &[("1035001".into(), "1035302".into())],
    )
    .unwrap();
    write_mvp_table(&game, &tables, &root, &[]).unwrap();
    write_gallery_table(&game, &tables, &root, &[]).unwrap();
    write_emote_tables(&game, &tables, &root, &[]).unwrap();
    let job = t.elapsed();
    let _ = std::fs::remove_dir_all(&root);
    println!("database build {build:?}, swap job {job:?}");
    assert!(
        build < Duration::from_secs(5),
        "database build took {build:?}"
    );
    assert!(job < Duration::from_secs(9), "swap job took {job:?}");
}
