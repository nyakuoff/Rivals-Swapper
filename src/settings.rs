//! User settings, compatible with the Python version's settings.json.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A deployed swap. Paths point at the files in ~mods.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct SwapRecord {
    pub skin_id: String,
    pub skin_name: String,
    pub mod_name: String,
    pub char_id: String,
    pub pak_path: String,
    pub utoc_path: String,
    pub ucas_path: String,
    /// The hero skin the swap replaces (the default skin).
    pub target_skin_id: String,
    /// Replaced meshes as (hero form, vanilla package path).
    pub meshes: Vec<(u8, String)>,
    /// The skin's ChildBP takes the default skin's row in the shared skin table.
    pub childbp: bool,
    /// The default MVP plays the skin's own MVP, whole scene (shared MVP table).
    pub mvp: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Settings {
    pub game_paks_dir: String,
    /// skin_id -> swap (several per hero allowed)
    pub active_swaps: BTreeMap<String, SwapRecord>,
    /// char_id -> MVP swap (one per hero; skin_id holds the MVP id)
    pub mvp_swaps: BTreeMap<String, SwapRecord>,
    /// char_id -> emote swap (one per hero; skin_id holds the emote id)
    pub emote_swaps: BTreeMap<String, SwapRecord>,
}

fn path() -> PathBuf {
    crate::config_dir().join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        let mut s: Settings = std::fs::read(path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // Releases up to 1.0.9 keyed swaps by char_id (one per hero); re-key by skin_id.
        s.active_swaps = std::mem::take(&mut s.active_swaps)
            .into_iter()
            .map(|(key, mut rec)| {
                if rec.skin_id.is_empty() || rec.skin_id == key {
                    return (key, rec);
                }
                if rec.char_id.is_empty() {
                    rec.char_id = key;
                }
                (rec.skin_id.clone(), rec)
            })
            .collect();
        s
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(crate::config_dir())?;
        std::fs::write(path(), serde_json::to_vec_pretty(self)?)
    }

    /// Whether the hero has any swap: a skin or an MVP.
    pub fn is_swapped(&self, char_id: &str) -> bool {
        self.swaps_for(char_id).next().is_some()
            || self.mvp_swaps.contains_key(char_id)
            || self.emote_swaps.contains_key(char_id)
    }

    pub fn swap_count(&self) -> usize {
        self.active_swaps.len() + self.mvp_swaps.len() + self.emote_swaps.len()
    }

    pub fn swaps_for(&self, char_id: &str) -> impl Iterator<Item = &SwapRecord> {
        self.active_swaps
            .values()
            .filter(move |r| r.char_id == char_id)
    }
}
