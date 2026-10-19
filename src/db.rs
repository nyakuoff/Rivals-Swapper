//! Hero/skin database, built straight from the game's Game.locres and cached
//! until the game files change. Replaces the old FModel + build_database.py step.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::game::{Game, SkinIndex};

/// A variant of a default cue: `<base>_<NNN>_BP`.
static VARIANT_CUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.*)_\d{3}_BP$").unwrap());

const LOCRES_PATH: &str = "Marvel/Content/Localization/Game/en/Game.locres";
/// Bump when the build rules change so old caches get rebuilt.
const DB_VERSION: u32 = 12;

/// Fixes for names the localization stores oddly: (char_id, display name).
const CHAR_NAME_OVERRIDES: &[(&str, &str)] = &[];

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Skin {
    pub id: String,
    pub name: String,
    /// Can be swapped: has its own mesh, or is a retexture of one.
    pub swappable: bool,
    /// No mesh of its own: swapped as its base mesh with this skin's materials.
    #[serde(default)]
    pub retexture: bool,
    /// Its sound table plays some of its own sound events (legendary skins).
    #[serde(default)]
    pub custom_sounds: bool,
    /// Has ability VFX (gameplay cues) of its own that can replace the default's.
    #[serde(default)]
    pub custom_effects: bool,
    /// Has variant cues (`<base>_<NNN>_BP`): effects the game picks from the
    /// skin's variant number, which a swap can't hand to the default skin, so
    /// those stay default. See docs/VFX_NOTES.md.
    #[serde(default)]
    pub missing_effects: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Character {
    pub name: String,
    pub char_id: String,
    pub default_skin_id: String,
    pub skins: Vec<Skin>,
    /// The default skin's blueprint points at its own body materials, which the
    /// hero's game logic applies to a part of whatever mesh is equipped. On a
    /// swapped mesh that part can come out with the default skin's texture.
    #[serde(default)]
    pub texture_risk: bool,
}

impl Character {
    /// Skins that can be picked as a swap source (no default look).
    pub fn alt_skins(&self) -> impl Iterator<Item = &Skin> {
        self.skins
            .iter()
            .filter(|s| !s.id.ends_with("001") && !s.id.ends_with("000"))
    }
}

#[derive(Serialize, Deserialize)]
struct Cache {
    version: u32,
    fingerprint: String,
    characters: Vec<Character>,
}

/// Size + mtime of every top-level file in Paks/: any game update changes it.
pub fn fingerprint(paks_dir: &Path) -> Result<String> {
    let mut parts = vec![];
    for entry in std::fs::read_dir(paks_dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            let mtime = meta
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            parts.push(format!(
                "{}:{}:{}",
                entry.file_name().to_string_lossy(),
                meta.len(),
                mtime
            ));
        }
    }
    parts.sort();
    Ok(parts.join("|"))
}

pub fn load_cached(cache_file: &Path, fingerprint: &str) -> Option<Vec<Character>> {
    let cache: Cache = serde_json::from_slice(&std::fs::read(cache_file).ok()?).ok()?;
    (cache.version == DB_VERSION && cache.fingerprint == fingerprint).then_some(cache.characters)
}

pub fn build_and_cache(
    game: &Game,
    cache_file: &Path,
    fingerprint: &str,
) -> Result<Vec<Character>> {
    let flat = parse_locres(&game.read_pak_file(LOCRES_PATH)?)?;
    let mut characters = build(&flat, game.skins());
    let work = std::env::temp_dir().join("rivals_swapper_bp_check");
    // Independent per hero, so check heroes in parallel (each in its own folder).
    characters.par_iter_mut().for_each(|c| {
        let work = work.join(&c.char_id);
        c.texture_risk = default_bp_uses_body_materials(game, &work, c);
        for s in &mut c.skins {
            s.custom_sounds = has_own_sounds(game, &work, &c.char_id, &s.id);
            let cues = &game.skins().cues;
            s.custom_effects = game.skins().vfx.contains(&s.id)
                || cues
                    .get(&s.id)
                    .zip(cues.get(&c.default_skin_id))
                    .is_some_and(|(own, default)| own.keys().any(|k| default.contains_key(k)));
            s.missing_effects = variant_cues_add_effects(game, &work, &s.id, &c.default_skin_id);
        }
    });
    let _ = std::fs::remove_dir_all(&work);
    let cache = Cache {
        version: DB_VERSION,
        fingerprint: fingerprint.to_string(),
        characters,
    };
    if let Some(dir) = cache_file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(cache_file, serde_json::to_vec_pretty(&cache)?)?;
    Ok(cache.characters)
}

/// Whether the skin has variant cues (`<base>_<NNN>_BP` of a default cue) that
/// spawn particle systems their base doesn't. The game picks those from the
/// skin's variant number, which a swap can't hand over, so they stay default.
/// (Most variant cues only change weapon materials or meshes.)
fn variant_cues_add_effects(game: &Game, work: &Path, skin: &str, default: &str) -> bool {
    let cues = &game.skins().cues;
    let (Some(own), Some(defaults)) = (cues.get(skin), cues.get(default)) else {
        return false;
    };
    let names = |p: &str| crate::swap::names(&game.extract_package(p, work).ok()?).ok();
    own.iter().any(|(rel, variant)| {
        let Some(base) = VARIANT_CUE
            .captures(rel)
            .and_then(|m| defaults.get(&format!("{}_BP", &m[1])))
        else {
            return false;
        };
        let (Some(v), Some(b)) = (names(variant), names(base)) else {
            return false;
        };
        v.iter()
            .any(|n| n.contains("/Particles/") && !b.contains(n))
    })
}

/// Whether the skin's sound table plays any event from its own Wwise folder.
fn has_own_sounds(game: &Game, work: &Path, hero: &str, skin: &str) -> bool {
    let Some(table) = game
        .skins()
        .sound_tables
        .get(skin)
        .and_then(|t| t.get("SoundTable"))
    else {
        return false;
    };
    let Ok(file) = game.extract_package(table, work) else {
        return false;
    };
    let own = format!("/WwiseEvents/Characters/{hero}/{skin}/");
    crate::swap::names(&file)
        .unwrap_or_default()
        .iter()
        .any(|n| n.contains(&own))
}

/// Whether the default skin's ChildBP references body materials of that skin
/// (weapon and VFX materials don't matter: they aren't on the body mesh).
fn default_bp_uses_body_materials(game: &Game, work: &Path, c: &Character) -> bool {
    let (h, d) = (&c.char_id, &c.default_skin_id);
    let Ok(bp) = game.extract_package(
        &format!("/Game/Marvel/Characters/{h}/{d}/{d}_ChildBP"),
        work,
    ) else {
        return false;
    };
    let body = format!("/Game/Marvel/Characters/{h}/{d}/Materials/");
    crate::swap::names(&bp)
        .unwrap_or_default()
        .iter()
        .any(|n| n.starts_with(&body) && !n.contains("Weapon") && !n.contains("Shield"))
}

// ---------------------------------------------------------------------------
// Locres
// ---------------------------------------------------------------------------

const LOCRES_MAGIC: [u8; 16] = [
    0x0E, 0x14, 0x74, 0x75, 0x67, 0x4A, 0x03, 0xFC, 0x4A, 0x15, 0x90, 0x9D, 0xC3, 0x37, 0x7F, 0x1B,
];

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Result<&[u8]> {
        let b = self
            .data
            .get(self.pos..self.pos + n)
            .context("locres truncated")?;
        self.pos += n;
        Ok(b)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.bytes(4)?.try_into()?))
    }
    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.bytes(8)?.try_into()?))
    }
    fn fstring(&mut self) -> Result<String> {
        let len = self.i32()?;
        Ok(if len < 0 {
            let units: Vec<u16> = self
                .bytes(len.unsigned_abs() as usize * 2)?
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
                .trim_end_matches('\0')
                .to_string()
        } else {
            String::from_utf8_lossy(self.bytes(len as usize)?)
                .trim_end_matches('\0')
                .to_string()
        })
    }
}

/// Flatten a .locres into key -> localized string (later namespaces win on duplicate keys).
pub fn parse_locres(data: &[u8]) -> Result<HashMap<String, String>> {
    let mut r = Reader { data, pos: 0 };
    if r.bytes(16)? != LOCRES_MAGIC {
        bail!("unsupported (legacy) locres format");
    }
    let version = r.u8()?;
    let strings_offset = r.i64()? as usize;
    if version >= 2 {
        r.i32()?; // entry count
    }
    let ns_pos = r.pos;

    r.pos = strings_offset;
    let count = r.i32()?;
    let mut strings = Vec::with_capacity(count.max(0) as usize);
    for _ in 0..count {
        strings.push(r.fstring()?);
        if version >= 2 {
            r.i32()?; // ref count
        }
    }

    r.pos = ns_pos;
    let mut flat = HashMap::new();
    for _ in 0..r.i32()? {
        if version >= 2 {
            r.i32()?; // namespace hash
        }
        r.fstring()?;
        for _ in 0..r.i32()? {
            if version >= 2 {
                r.i32()?; // key hash
            }
            let key = r.fstring()?;
            r.i32()?; // source string hash
            let idx = r.i32()?;
            if let Some(s) = strings.get(idx as usize) {
                flat.insert(key, s.clone());
            }
        }
    }
    Ok(flat)
}

// ---------------------------------------------------------------------------
// Build rules (ported from scripts/build_database.py)
// ---------------------------------------------------------------------------

fn re(s: &str) -> Regex {
    Regex::new(s).unwrap()
}
static RE_SKIN_NAME: LazyLock<Regex> =
    LazyLock::new(|| re(r"^UISkinTable_(\d+)_SkinBasic_SkinName$"));
static RE_SKIN_NAME2: LazyLock<Regex> =
    LazyLock::new(|| re(r"^HeroUIAssetBPTable_(\d{8})_SkinInfo_SkinName$"));
// Platform exclusives carry a prefix, e.g. MarvelItemTable_ps1048307_ItemName.
static RE_SKIN_ITEM: LazyLock<Regex> =
    LazyLock::new(|| re(r"^MarvelItemTable_[a-z]*(\d{7})_ItemName$"));
// Some skins are only named in their store banner: "LADY LOKI & JACK OF HEARTS".
static RE_NEW_ARRIVAL: LazyLock<Regex> =
    LazyLock::new(|| re(r"^TXT_NewArrival_Title_(\d{7}(?:and\d{7})*)$"));
static RE_HERO_TNAME1: LazyLock<Regex> =
    LazyLock::new(|| re(r"^HeroUIAssetBPTable_(\d{8})_HeroInfo_TName$"));
static RE_HERO_TNAME2: LazyLock<Regex> =
    LazyLock::new(|| re(r"^UIHeroTable_(\d{5})_HeroBasic_TName$"));
static RE_HERO_ITEM: LazyLock<Regex> = LazyLock::new(|| re(r"^MarvelItemTable_(\d{4})_ItemName$"));
static RE_NAMEPLATE: LazyLock<Regex> =
    LazyLock::new(|| re(r"^UIHeroNameplateTable_3(\d{4})001_Name$"));

/// "ANTI-VENOM" -> "Anti-Venom", "NYX WEAVER" -> "Nyx Weaver"
fn title(s: &str) -> String {
    s.split_whitespace()
        .map(|part| {
            part.split('-')
                .map(|w| {
                    let mut c = w.chars();
                    c.next()
                        .map(|f| {
                            f.to_uppercase()
                                .chain(c.flat_map(char::to_lowercase))
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .collect::<Vec<String>>()
                .join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Capture group 1 of `re` for every matching key, as (capture, value), in key
/// order so results don't depend on hash-map iteration.
fn matches<'a>(flat: &'a HashMap<String, String>, re: &'a Regex) -> Vec<(&'a str, &'a str)> {
    let mut keys: Vec<&String> = flat.keys().collect();
    keys.sort();
    keys.into_iter()
        .filter_map(|k| Some((re.captures(k)?.get(1)?.as_str(), flat[k].as_str())))
        .collect()
}

/// 0 = real name, 1 = internal placeholder ("[sg] … - Chroma 2"),
/// 2 = untranslated (unreleased skins only have Chinese names).
fn placeholder_rank(name: &str) -> u8 {
    if name
        .chars()
        .any(|c| ('\u{3000}'..='\u{9fff}').contains(&c) || ('\u{ff00}'..='\u{ffef}').contains(&c))
    {
        2
    } else if name.starts_with('[') || name.to_lowercase().contains("chroma") {
        1
    } else {
        0
    }
}

/// skin_id (7 digits) -> name. Several keys can name a skin; real names beat
/// placeholders, then UISkinTable beats the store name (MarvelItemTable), then
/// SkinInfo (some pre-release names), then store banners; within a source the
/// last key wins. Skins with only untranslated names are left out.
fn skin_names(flat: &HashMap<String, String>) -> HashMap<String, String> {
    // id -> (placeholder rank, source, key order, name), best = smallest
    let mut best: HashMap<String, (u8, u8, std::cmp::Reverse<usize>, String)> = HashMap::new();
    let mut order = 0;
    let mut offer = |id: String, source: u8, name: &str| {
        order += 1;
        let cand = (
            placeholder_rank(name),
            source,
            std::cmp::Reverse(order),
            title(name),
        );
        match best.get(&id) {
            Some(cur) if *cur <= cand => {}
            _ => {
                best.insert(id, cand);
            }
        }
    };
    for (id, v) in matches(flat, &RE_SKIN_NAME) {
        offer((id.parse::<u64>().unwrap_or(0) / 10).to_string(), 0, v);
    }
    for (id, v) in matches(flat, &RE_SKIN_ITEM) {
        if id.starts_with("10") {
            offer(id.to_string(), 1, v);
        }
    }
    for (id, v) in matches(flat, &RE_SKIN_NAME2) {
        offer((id.parse::<u64>().unwrap_or(0) / 10).to_string(), 2, v);
    }
    for (ids, v) in matches(flat, &RE_NEW_ARRIVAL) {
        let (ids, names): (Vec<_>, Vec<_>) = (ids.split("and").collect(), v.split(" & ").collect());
        if ids.len() == names.len() {
            for (id, name) in ids.into_iter().zip(names) {
                offer(id.to_string(), 3, name);
            }
        }
    }
    best.into_iter()
        .filter(|(_, c)| c.0 < 2)
        .map(|(id, c)| (id, c.3))
        .collect()
}

/// char_id (4 digits) -> name. Nameplate is authoritative, overrides beat everything.
fn char_names(flat: &HashMap<String, String>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (id, v) in matches(flat, &RE_HERO_TNAME1) {
        out.entry(id[..4].to_string()).or_insert_with(|| title(v));
    }
    for (id, v) in matches(flat, &RE_HERO_TNAME2) {
        out.entry((id.parse::<u64>().unwrap_or(0) / 10).to_string())
            .or_insert_with(|| title(v));
    }
    for (id, v) in matches(flat, &RE_NAMEPLATE) {
        out.insert(id.to_string(), title(v));
    }
    for (id, v) in matches(flat, &RE_HERO_ITEM) {
        out.entry(id.to_string()).or_insert_with(|| title(v));
    }
    for (id, name) in CHAR_NAME_OVERRIDES {
        out.insert(id.to_string(), name.to_string());
    }
    out
}

/// Every skin with a mesh in the game files is listed, even before the
/// localization names it.
pub fn build(flat: &HashMap<String, String>, index: &SkinIndex) -> Vec<Character> {
    let names = char_names(flat);
    let mut skin_names = skin_names(flat);
    for id in index.meshes.keys() {
        skin_names
            .entry(id.clone())
            .or_insert_with(|| format!("Skin {id}"));
    }
    let mut skins_by_char: HashMap<String, Vec<Skin>> = HashMap::new();
    for (id, name) in skin_names {
        if id.len() >= 4 {
            let own_mesh = index.meshes.contains_key(&id);
            let retexture = !own_mesh && index.blueprints.contains(&id);
            skins_by_char
                .entry(id[..4].to_string())
                .or_default()
                .push(Skin {
                    id,
                    name,
                    swappable: own_mesh || retexture,
                    custom_sounds: false,
                    custom_effects: false,
                    missing_effects: false,
                    retexture,
                });
        }
    }

    // Playable heroes are 10xx; NPCs/bots are 4xxx, 9999, ...
    let mut by_name: HashMap<String, Character> = HashMap::new();
    for (char_id, skins) in skins_by_char {
        if char_id.len() != 4 || !char_id.starts_with("10") {
            continue;
        }
        let Some(name) = names.get(&char_id) else {
            continue;
        };
        let default_skin_id = format!("{char_id}001");
        // Several char_ids can share one name (e.g. Cloak & Dagger): merge their skins.
        let c = by_name.entry(name.clone()).or_insert_with(|| Character {
            name: name.clone(),
            char_id: char_id.clone(),
            default_skin_id,
            skins: vec![],
            texture_risk: false,
        });
        for s in skins {
            if !c.skins.iter().any(|x| x.id == s.id) {
                c.skins.push(s);
            }
        }
    }
    let mut characters: Vec<Character> = by_name.into_values().collect();
    for c in &mut characters {
        let default = c.default_skin_id.clone();
        c.skins
            .sort_by(|a, b| (a.id != default, &a.id).cmp(&(b.id != default, &b.id)));
    }
    characters.sort_by(|a, b| a.name.cmp(&b.name));
    characters
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fstring(s: &str) -> Vec<u8> {
        let mut v = ((s.len() + 1) as i32).to_le_bytes().to_vec();
        v.extend(s.as_bytes());
        v.push(0);
        v
    }

    /// Minimal version-3 locres with one namespace.
    fn locres(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut ns = vec![];
        ns.extend(1i32.to_le_bytes());
        ns.extend(0i32.to_le_bytes());
        ns.extend(fstring(""));
        ns.extend((entries.len() as i32).to_le_bytes());
        for (i, (k, _)) in entries.iter().enumerate() {
            ns.extend(0i32.to_le_bytes());
            ns.extend(fstring(k));
            ns.extend(0i32.to_le_bytes());
            ns.extend((i as i32).to_le_bytes());
        }
        let mut out = LOCRES_MAGIC.to_vec();
        out.push(3);
        let strings_offset = out.len() + 8 + 4 + ns.len();
        out.extend((strings_offset as i64).to_le_bytes());
        out.extend((entries.len() as i32).to_le_bytes());
        out.extend(ns);
        out.extend((entries.len() as i32).to_le_bytes());
        for (_, v) in entries {
            out.extend(fstring(v));
            out.extend(1i32.to_le_bytes());
        }
        out
    }

    #[test]
    fn builds_from_locres() {
        let data = locres(&[
            ("UIHeroNameplateTable_31035001_Name", "VENOM"),
            ("HeroUIAssetBPTable_10350010_HeroInfo_TName", "Wrong Name"),
            ("UISkinTable_10351010_SkinBasic_SkinName", "ANTI-VENOM"),
            (
                "HeroUIAssetBPTable_10351010_SkinInfo_SkinName",
                "Loses To UISkinTable",
            ),
            ("HeroUIAssetBPTable_10350010_SkinInfo_SkinName", "VENOM"),
            ("MarvelItemTable_1035300_ItemName", "SYMBIOTE KING"),
            (
                "HeroUIAssetBPTable_10353000_SkinInfo_SkinName",
                "Loses To ItemTable",
            ),
            ("MarvelItemTable_ps1035307_ItemName", "MATCHA VENOM"),
            (
                "TXT_NewArrival_Title_1016502and1035500",
                "LADY LOKI & JACK OF VENOM",
            ),
            ("UISkinTable_10355010_SkinBasic_SkinName", "[sg]毒液-换色"),
            (
                "UISkinTable_10358000_SkinBasic_SkinName",
                "MARVEL COSMIC INVASION",
            ),
            ("UISkinTable_40010010_SkinBasic_SkinName", "Bot Skin"),
        ]);
        let mesh = |id: &str| (id.to_string(), vec![(0, String::new())]);
        let index = SkinIndex {
            meshes: ["1035001", "1035300", "1035800", "1035900"]
                .map(mesh)
                .into(),
            blueprints: ["1035101", "1035300"].map(String::from).into(),
            ..Default::default()
        };
        let chars = build(&parse_locres(&data).unwrap(), &index);
        assert_eq!(chars.len(), 1, "{chars:?}");
        let venom = &chars[0];
        assert_eq!(
            (venom.name.as_str(), venom.char_id.as_str()),
            ("Venom", "1035")
        );
        let skins: Vec<_> = venom
            .skins
            .iter()
            .map(|s| (s.id.as_str(), s.name.as_str(), s.swappable, s.retexture))
            .collect();
        assert_eq!(
            skins,
            [
                ("1035001", "Venom", true, false),
                ("1035101", "Anti-Venom", true, true), // blueprint, no mesh
                ("1035300", "Symbiote King", true, false),
                ("1035307", "Matcha Venom", false, false),
                ("1035500", "Jack Of Venom", false, false),
                ("1035800", "Marvel Cosmic Invasion", true, false),
                ("1035900", "Skin 1035900", true, false), // mesh without a name yet
            ]
        );
        assert_eq!(venom.alt_skins().count(), 6);
    }
}
