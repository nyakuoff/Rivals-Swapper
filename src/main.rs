#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod db;
mod game;
mod images;
mod settings;
mod swap;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Stroke, Vec2};

use db::{Character, Emote, Mvp, Skin};
use game::Game;
use settings::{Settings, SwapRecord};

const GITHUB_REPO: &str = "nyakuoff/Rivals-Swapper";
const STEAM_URL: &str = "steam://run/2767030";

// ---------------------------------------------------------------------------
// Paths and process helpers
// ---------------------------------------------------------------------------

/// tools/ next to the binary (or above it, for `cargo run`).
pub fn tools_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.ancestors()
        .map(|d| d.join("tools"))
        .find(|t| t.is_dir())
        .unwrap_or_else(|| exe.with_file_name("tools"))
}

pub fn uassettool() -> PathBuf {
    tools_dir().join("uassettool").join(if cfg!(windows) {
        "UAssetTool.exe"
    } else {
        "UAssetTool"
    })
}

fn user_dir(windows_var: &str, xdg_var: &str, xdg_default: &str) -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os(windows_var).map(PathBuf::from)
    } else {
        std::env::var_os(xdg_var)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let home = || {
        PathBuf::from(
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .unwrap_or_default(),
        )
    };
    base.unwrap_or_else(|| home().join(xdg_default))
        .join("RivalsSwapper")
}

/// Settings live here (same place as the Python version).
pub fn config_dir() -> PathBuf {
    user_dir("APPDATA", "XDG_CONFIG_HOME", ".config")
}

/// Regenerable data: skin database, images, temp work.
pub fn cache_dir() -> PathBuf {
    user_dir("LOCALAPPDATA", "XDG_CACHE_HOME", ".cache")
}

/// A Command that doesn't flash a console window on Windows.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

/// Every file under `dir`, recursively.
pub fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.extend(walk(&p))
        } else {
            out.push(p)
        }
    }
    out
}

/// Find Marvel Rivals' Paks folder in the usual Steam libraries.
fn find_game_paks() -> Option<PathBuf> {
    let game_rel = "steamapps/common/MarvelRivals/MarvelGame/Marvel/Content/Paks";
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut roots: Vec<PathBuf> = if cfg!(windows) {
        vec![
            "C:/Program Files (x86)/Steam".into(),
            "C:/Program Files/Steam".into(),
        ]
    } else {
        [
            ".steam/steam",
            ".local/share/Steam",
            "Steam",
            ".var/app/com.valvesoftware.Steam/.local/share/Steam",
        ]
        .iter()
        .map(|r| home.join(r))
        .collect()
    };
    // libraryfolders.vdf lists the extra Steam libraries.
    let re = regex::Regex::new(r#""path"\s+"([^"]+)""#).unwrap();
    for root in roots.clone() {
        if let Ok(text) = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
            roots.extend(
                re.captures_iter(&text)
                    .map(|m| PathBuf::from(m[1].replace("\\\\", "\\"))),
            );
        }
    }
    roots
        .into_iter()
        .map(|r| r.join(game_rel))
        .find(|p| p.is_dir())
}

/// The game's container indexes take ~750 MB, so `Game` is opened per operation
/// (database rebuild, swap) and dropped; this hands the memory back to the OS,
/// which glibc otherwise keeps.
fn release_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::malloc_trim(0);
    }
}

fn is_paks_dir(p: &Path) -> bool {
    std::fs::read_dir(p)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.path().extension().is_some_and(|x| x == "utoc"))
}

fn latest_release() -> Option<(String, String)> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let v: serde_json::Value = ureq::get(&url)
        .header("User-Agent", "RivalsSwapper")
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    Some((
        v["tag_name"].as_str()?.to_string(),
        v["html_url"].as_str()?.to_string(),
    ))
}

fn parse_version(v: &str) -> Vec<u32> {
    v.trim_start_matches('v')
        .split('.')
        .map(|x| x.parse().unwrap_or(0))
        .collect()
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag, out] = args.as_slice()
        && flag == "--dump-db"
    {
        if let Err(e) = dump_db(Path::new(out)) {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    if let Err(e) = install_desktop_entry() {
        eprintln!("[desktop entry] {e}");
    }
    let icon =
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/RivalsIcon_Outline.png"))
            .unwrap_or_default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Rivals Swapper")
            .with_inner_size([1240.0, 800.0])
            .with_min_inner_size([960.0, 600.0])
            .with_icon(icon)
            // Wayland shows the icon of the .desktop entry matching this id.
            .with_app_id("rivals-swapper"),
        ..Default::default()
    };
    eframe::run_native(
        "Rivals Swapper",
        options,
        Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))),
    )
}

/// Wayland compositors take window icons from the .desktop entry whose name
/// matches the app id, so register one for this binary (and its icon).
#[cfg(target_os = "linux")]
fn install_desktop_entry() -> std::io::Result<()> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        });
    let icon = data.join("icons/hicolor/512x512/apps/rivals-swapper.png");
    let entry = data.join("applications/rivals-swapper.desktop");
    let exe = std::env::current_exe()?;
    let text = format!(
        "[Desktop Entry]\nType=Application\nName=Rivals Swapper\nComment=Swap Marvel Rivals skins\nExec=\"{}\"\nIcon=rivals-swapper\nStartupWMClass=rivals-swapper\nCategories=Game;Utility;\n",
        exe.display()
    );
    if std::fs::read_to_string(&entry).ok().as_deref() != Some(text.as_str()) {
        std::fs::create_dir_all(icon.parent().unwrap())?;
        std::fs::write(&icon, include_bytes!("../assets/RivalsIcon_Outline.png"))?;
        std::fs::create_dir_all(entry.parent().unwrap())?;
        std::fs::write(&entry, text)?;
    }
    Ok(())
}

/// `rivals-swapper --dump-db out.json`: rebuild the database from the game and write it.
fn dump_db(out: &Path) -> anyhow::Result<()> {
    let settings = Settings::load();
    let paks = Some(PathBuf::from(&settings.game_paks_dir))
        .filter(|p| is_paks_dir(p))
        .or_else(find_game_paks)
        .ok_or_else(|| anyhow::anyhow!("Paks folder not found"))?;
    let game = Game::open(&paks)?;
    let fp = db::fingerprint(&paks)?;
    let chars = db::build_and_cache(&game, &cache_dir().join("db.json"), &fp)?;
    std::fs::write(out, serde_json::to_vec_pretty(&chars)?)?;
    println!(
        "{} characters, {} skins -> {}",
        chars.len(),
        chars.iter().map(|c| c.skins.len()).sum::<usize>(),
        out.display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

// Palette from the game's menus: indigo night sky, logo yellow, Marvel red.
const VOID: Color32 = Color32::from_rgb(0x0e, 0x10, 0x20);
const PANEL: Color32 = Color32::from_rgb(0x15, 0x18, 0x31);
const RAISED: Color32 = Color32::from_rgb(0x1e, 0x22, 0x40);
const LINE: Color32 = Color32::from_rgb(0x2c, 0x31, 0x58);
const YELLOW: Color32 = Color32::from_rgb(0xff, 0xc7, 0x2c);
const RED: Color32 = Color32::from_rgb(0xe5, 0x30, 0x3f);
const TEXT: Color32 = Color32::from_rgb(0xed, 0xef, 0xf7);
const MUTED: Color32 = Color32::from_rgb(0x8a, 0x90, 0xb2);
/// Dark text on yellow.
const INK: Color32 = Color32::from_rgb(0x14, 0x16, 0x2a);

/// State shared with worker threads.
#[derive(Default)]
struct Shared {
    settings: Settings,
    characters: Vec<Character>,
    /// Background activity shown in the top bar.
    status: Option<String>,
    /// Blocking problem (no Paks folder, unreadable game files).
    error: Option<String>,
    log: Vec<String>,
    busy: bool,
    update: Option<(String, String)>,
    images_changed: bool,
}

#[derive(Clone)]
struct Handle {
    shared: Arc<Mutex<Shared>>,
    ctx: egui::Context,
}

impl Handle {
    fn with<R>(&self, f: impl FnOnce(&mut Shared) -> R) -> R {
        let r = f(&mut self.shared.lock().unwrap());
        self.ctx.request_repaint();
        r
    }
    fn log(&self, msg: String) {
        self.with(|s| s.log.push(msg));
    }

    /// Run `job` on a worker thread unless one is already running.
    fn run(&self, job: impl FnOnce(&Handle) + Send + 'static) {
        if self.with(|s| std::mem::replace(&mut s.busy, true)) {
            return;
        }
        let h = self.clone();
        std::thread::spawn(move || {
            job(&h);
            h.with(|s| s.busy = false);
        });
    }

    /// Find the game, load or rebuild the database, refresh images.
    fn load(&self) {
        let h = self.clone();
        std::thread::spawn(move || {
            let mut paks = h.with(|s| s.settings.game_paks_dir.clone());
            if !is_paks_dir(Path::new(&paks)) {
                match find_game_paks() {
                    Some(p) => {
                        paks = p.to_string_lossy().into_owned();
                        h.with(|s| {
                            s.settings.game_paks_dir = paks.clone();
                            let _ = s.settings.save();
                        });
                    }
                    None => {
                        return h.with(|s| {
                            s.error = Some("Set your Marvel Rivals Paks folder in Settings.".into())
                        });
                    }
                }
            }
            let paks = PathBuf::from(paks);
            let status = |msg: &str| h.with(|s| s.status = Some(msg.into()));
            let res = (|| -> anyhow::Result<String> {
                let fp = db::fingerprint(&paks)?;
                let cache = cache_dir().join("db.json");
                let chars = match db::load_cached(&cache, &fp) {
                    Some(chars) => chars,
                    None => {
                        status("Game updated: rebuilding skin database…");
                        let chars = db::build_and_cache(&Game::open(&paks)?, &cache, &fp)?;
                        release_memory();
                        h.log(format!(
                            "Skin database rebuilt: {} heroes, {} skins",
                            chars.len(),
                            chars.iter().map(|c| c.skins.len()).sum::<usize>()
                        ));
                        chars
                    }
                };
                h.with(|s| s.characters = chars);
                Ok(fp)
            })();
            match res {
                Ok(fp) if !images::up_to_date(&fp) => {
                    let set = |msg: String| h.with(|s| s.status = Some(msg));
                    match images::export(&paks, &fp, &set) {
                        Ok(()) => {
                            release_memory();
                            h.with(|s| s.images_changed = true);
                        }
                        Err(e) => h.log(format!("[WARN] Couldn't export skin images: {e:#}")),
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    h.with(|s| s.error = Some(format!("Couldn't read the game files: {e:#}")))
                }
            }
            h.with(|s| s.status = None);
        });
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Skins,
    Mvp,
    Emotes,
}

struct App {
    h: Handle,
    /// char_id of the hero on stage.
    selected: Option<String>,
    search: String,
    swapped_only: bool,
    log_open: bool,
    settings_open: bool,
    paks_input: String,
    confirm_remove_all: bool,
    /// Skin waiting on the swap options prompt, with the choices so far.
    swap_prompt: Option<(Character, Skin, swap::SwapOptions)>,
    /// Which list the hero view shows.
    tab: Tab,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        egui_extras::install_image_loaders(ctx);
        fonts(ctx);
        style(ctx);
        let settings = Settings::load();
        let h = Handle {
            shared: Arc::new(Mutex::new(Shared {
                settings,
                ..Default::default()
            })),
            ctx: ctx.clone(),
        };
        h.load();
        let hc = h.clone();
        std::thread::spawn(move || {
            if let Some((tag, url)) = latest_release()
                && parse_version(&tag) > parse_version(env!("CARGO_PKG_VERSION"))
            {
                hc.with(|s| s.update = Some((tag, url)));
            }
        });
        Self {
            h,
            selected: None,
            search: String::new(),
            swapped_only: false,
            log_open: false,
            settings_open: false,
            paks_input: String::new(),
            confirm_remove_all: false,
            swap_prompt: None,
            tab: Tab::Skins,
        }
    }

    fn start_swap(&mut self, character: Character, skin: Skin, options: swap::SwapOptions) {
        self.log_open = true;
        self.h.with(|s| s.log.clear());
        self.h.run(move |h| {
            h.log(format!(
                "Swapping {} -> {} ({})",
                character.name, skin.name, skin.id
            ));
            if character.texture_risk {
                h.log(format!("[WARN] {}", texture_risk_text(&character.name)));
            }
            let paks = PathBuf::from(h.with(|s| s.settings.game_paks_dir.clone()));
            let log = |m: String| h.log(format!("  {m}"));
            log("Reading game files...".into());
            let res = Game::open(&paks).and_then(|game| {
                swap::create(
                    &game,
                    &cache_dir().join("work"),
                    &paks.join("~mods"),
                    &character,
                    &skin,
                    options,
                    &log,
                )
            });
            release_memory();
            match res {
                Ok(record) => h.with(|s| {
                    // One swap per hero: they all replace the same default mesh.
                    let replaced: Vec<String> = s
                        .settings
                        .swaps_for(&record.char_id)
                        .filter(|r| r.skin_id != record.skin_id)
                        .map(|r| r.skin_id.clone())
                        .collect();
                    for id in replaced {
                        delete_swap(&mut s.settings, &id, &mut s.log);
                    }
                    s.settings
                        .active_swaps
                        .insert(record.skin_id.clone(), record);
                    let _ = s.settings.save();
                    s.log.push(format!(
                        "Done! Equip {}'s default skin in-game and {} shows up.",
                        character.name, skin.name
                    ));
                }),
                Err(e) => return h.log(format!("[ERROR] Swap failed: {e:#}")),
            }
            update_tables(h);
        });
    }

    /// Make `mvp` the hero's default MVP (one per hero), through the MVP table.
    fn start_mvp_swap(&mut self, character: Character, mvp: Mvp) {
        self.log_open = true;
        let label = mvp_label(&character, &mvp);
        self.h.with(|s| {
            s.log.clear();
            s.log.push(format!(
                "Swapping {}'s default MVP -> {label} ({})",
                character.name, mvp.id
            ));
            let record = SwapRecord {
                skin_id: mvp.id.clone(),
                skin_name: label.clone(),
                char_id: character.char_id.clone(),
                ..Default::default()
            };
            s.settings
                .mvp_swaps
                .insert(character.char_id.clone(), record);
            let _ = s.settings.save();
        });
        self.h.run(move |h| {
            update_tables(h);
            h.log(format!(
                "Done! {}'s default MVP is now {label}.",
                character.name
            ));
        });
    }

    /// Make `emote` the hero's default emote (one per hero), through the emote tables.
    fn start_emote_swap(&mut self, character: Character, emote: Emote) {
        self.log_open = true;
        let label = emote_label(&emote);
        self.h.with(|s| {
            s.log.clear();
            s.log.push(format!(
                "Swapping {}'s default emote -> {label} ({})",
                character.name, emote.id
            ));
            let record = SwapRecord {
                skin_id: emote.id.clone(),
                skin_name: label.clone(),
                char_id: character.char_id.clone(),
                ..Default::default()
            };
            s.settings
                .emote_swaps
                .insert(character.char_id.clone(), record);
            let _ = s.settings.save();
        });
        self.h.run(move |h| {
            update_tables(h);
            h.log(format!(
                "Done! {}'s default emote is now {label}.",
                character.name
            ));
        });
    }

    fn remove_emote_swap(&mut self, char_id: &str) {
        self.h.with(|s| {
            if let Some(rec) = s.settings.emote_swaps.remove(char_id) {
                s.log.push(format!("Removed {}", rec.skin_name));
            }
            let _ = s.settings.save();
        });
        self.h.run(update_tables);
    }

    /// Emotes tab: any of the hero's emotes can replace the default emote.
    fn emote_view(&mut self, ui: &mut egui::Ui, c: &Character, busy: bool, settings: &Settings) {
        let current = settings.emote_swaps.get(&c.char_id);
        let mut action: Option<Result<Emote, ()>> = None; // Ok = swap, Err = remove
        ui.horizontal(|ui| match current {
            Some(rec) => {
                ui.label(
                    RichText::new(format!("Default emote plays {}.", rec.skin_name)).color(TEXT),
                );
                if slant_button(ui, "Remove emote swap", Tone::Ghost, !busy, 0.0).clicked() {
                    action = Some(Err(()));
                }
            }
            None => {
                ui.label(
                    RichText::new("Default emote. Pick an emote to play in its place.")
                        .color(MUTED),
                );
            }
        });
        ui.add_space(10.0);
        let emotes: Vec<&Emote> = c.emotes.iter().filter(|e| e.is_choice()).collect();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                rows(ui, &emotes, |ui, emote, width| {
                    let active = current.is_some_and(|r| r.skin_id == emote.id);
                    // The emote's icon; without one, its skin's art or the hero's portrait.
                    let skin = &emote.id[..7];
                    let image = [images::emote(&emote.id), images::skin(skin)]
                        .into_iter()
                        .find(|p| p.exists())
                        .unwrap_or_else(|| images::portrait(&c.char_id));
                    card(ui, width, &image, false, active, |ui| {
                        let label = emote_label(emote);
                        ui.add(
                            egui::Label::new(RichText::new(&label).font(font("medium", 15.5)))
                                .truncate(),
                        )
                        .on_hover_text(&label);
                        let note = match c.skins.iter().find(|s| s.id == skin) {
                            Some(s) if s.id == c.default_skin_id => {
                                "Default skin emote".to_string()
                            }
                            Some(s) => format!("{} emote", s.name),
                            None => "Shared emote".to_string(),
                        };
                        card_note(
                            ui,
                            &note,
                            "Plays in place of the default emote, in matches and in the lobby.",
                        );
                        ui.add_space(2.0);
                        let w = ui.available_width();
                        if active {
                            if slant_button(ui, "Remove", Tone::Danger, !busy, w).clicked() {
                                action = Some(Err(()));
                            }
                        } else {
                            let label = if current.is_some() {
                                "Swap instead"
                            } else {
                                "Swap"
                            };
                            if slant_button(ui, label, Tone::Primary, !busy, w).clicked() {
                                action = Some(Ok((*emote).clone()));
                            }
                        }
                    });
                });
            });
        match action {
            Some(Ok(emote)) => self.start_emote_swap(c.clone(), emote),
            Some(Err(())) => self.remove_emote_swap(&c.char_id),
            None => {}
        }
    }

    fn remove_mvp_swap(&mut self, char_id: &str) {
        self.h.with(|s| {
            if let Some(rec) = s.settings.mvp_swaps.remove(char_id) {
                s.log.push(format!("Removed {}", rec.skin_name));
            }
            let _ = s.settings.save();
        });
        self.h.run(update_tables);
    }

    fn mvp_view(&mut self, ui: &mut egui::Ui, c: &Character, busy: bool, settings: &Settings) {
        let current = settings.mvp_swaps.get(&c.char_id);
        let mut action: Option<Result<Mvp, ()>> = None; // Ok = swap, Err = remove
        ui.horizontal(|ui| match current {
            Some(rec) => {
                ui.label(
                    RichText::new(format!("Default MVP plays {}.", rec.skin_name)).color(TEXT),
                );
                if slant_button(ui, "Remove MVP swap", Tone::Ghost, !busy, 0.0).clicked() {
                    action = Some(Err(()));
                }
            }
            None => {
                ui.label(
                    RichText::new("Default MVP. Pick an MVP to play in its place. A swapped skin brings its own version of the default MVP.").color(MUTED),
                );
            }
        });
        ui.add_space(10.0);
        // A skin's version of the default MVP comes with that skin's swap.
        let mvps: Vec<&Mvp> = c.mvps.iter().filter(|m| m.is_special()).collect();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                rows(ui, &mvps, |ui, mvp, width| {
                    let active = current.is_some_and(|r| r.skin_id == mvp.id);
                    card(
                        ui,
                        width,
                        &images::skin(&mvp.id[..7]),
                        false,
                        active,
                        |ui| {
                            let label = mvp_label(c, mvp);
                            ui.add(
                                egui::Label::new(RichText::new(&label).font(font("medium", 15.5)))
                                    .truncate(),
                            )
                            .on_hover_text(&label);
                            let (note, tip) = if mvp.own_stage {
                                (
                                    "Special MVP, own stage",
                                    "Plays in place of the default MVP, with its own stage.",
                                )
                            } else {
                                ("Special MVP", "Plays in place of the default MVP.")
                            };
                            card_note(ui, note, tip);
                            ui.add_space(2.0);
                            let w = ui.available_width();
                            if active {
                                if slant_button(ui, "Remove", Tone::Danger, !busy, w).clicked() {
                                    action = Some(Err(()));
                                }
                            } else {
                                let label = if current.is_some() {
                                    "Swap instead"
                                } else {
                                    "Swap"
                                };
                                if slant_button(ui, label, Tone::Primary, !busy, w).clicked() {
                                    action = Some(Ok((*mvp).clone()));
                                }
                            }
                        },
                    );
                });
            });
        match action {
            Some(Ok(mvp)) => self.start_mvp_swap(c.clone(), mvp),
            Some(Err(())) => self.remove_mvp_swap(&c.char_id),
            None => {}
        }
    }

    fn remove_swaps(&mut self, skin_ids: Vec<String>) {
        self.h.with(|s| {
            for id in skin_ids {
                delete_swap(&mut s.settings, &id, &mut s.log);
            }
            let _ = s.settings.save();
        });
        self.h.run(update_tables);
    }

    fn launch(&mut self) {
        self.log_open = true;
        self.h.log("Launching Marvel Rivals...".into());
        if let Err(e) = open::that(STEAM_URL) {
            self.h.log(format!("[ERROR] Couldn't open Steam: {e}"));
        }
    }
}

/// Rewrite the shared skin-table mod from the active swaps (see
/// Rewrite the shared table mods from the active swaps (or remove them when
/// nothing needs them): the skin table (skin blueprints, see
/// `swap::write_skin_table`) and the MVP table (each hero's MVP tab choice, or
/// the swapped skin's version of the default MVP, see `swap::write_mvp_table`).
fn update_tables(h: &Handle) {
    let (paks, blueprints, mvps, gallery, emotes) = h.with(|s| {
        // (default skin, emote id) per hero with an emote swap.
        let emotes: Vec<(String, String)> = s
            .characters
            .iter()
            .filter_map(|c| {
                s.settings
                    .emote_swaps
                    .get(&c.char_id)
                    .map(|r| (c.default_skin_id.clone(), r.skin_id.clone()))
            })
            .collect();
        let blueprints: Vec<(String, String)> = s
            .settings
            .active_swaps
            .values()
            .filter(|r| r.childbp)
            .map(|r| (r.target_skin_id.clone(), r.skin_id.clone()))
            .collect();
        let mut mvps = vec![];
        // Skins whose MVP plays on the default skin: their show actors come along.
        let mut gallery = vec![];
        for c in &s.characters {
            let sequence = |id: &str| {
                c.mvps
                    .iter()
                    .find(|m| m.id == id)
                    .map(|m| m.package.clone())
            };
            let default_id = format!("{}301", c.default_skin_id);
            let Some(default_sequence) = sequence(&default_id) else {
                continue;
            };
            let swapped = s.settings.swaps_for(&c.char_id).next();
            // The MVP tab's choice, else the swapped skin's own MVP if ticked.
            let chosen = s
                .settings
                .mvp_swaps
                .get(&c.char_id)
                .map(|r| r.skin_id.clone())
                .or_else(|| {
                    swapped
                        .filter(|r| r.mvp)
                        .and_then(|r| c.own_mvp(&r.skin_id))
                        .map(|m| m.id.clone())
                });
            if let Some(mvp_id) = chosen {
                // The swapped skin's take on the MVP if it has one, else the MVP's own skin's.
                let skins = swapped
                    .map(|r| r.skin_id.clone())
                    .into_iter()
                    .chain([mvp_id[..7].to_string()])
                    .collect();
                // Show actors (lobby, gallery, the MVP's own shots) stay the swapped
                // skin's; all of a hero's show actors share the default's parts, so the
                // MVP plays with them. No skin swap: the MVP's own skin.
                let shown = swapped.map_or_else(|| mvp_id[..7].to_string(), |r| r.skin_id.clone());
                gallery.push((c.default_skin_id.clone(), shown));
                mvps.push(swap::MvpChoice::Special {
                    default_id,
                    mvp_id,
                    default_skin: c.default_skin_id.clone(),
                    skins,
                });
            } else if let Some((r, skin_sequence)) =
                swapped.and_then(|r| sequence(&format!("{}301", r.skin_id)).map(|q| (r, q)))
            {
                gallery.push((c.default_skin_id.clone(), r.skin_id.clone()));
                mvps.push(swap::MvpChoice::SkinVersion {
                    default_sequence,
                    skin_sequence,
                });
            }
        }
        (
            PathBuf::from(&s.settings.game_paks_dir),
            blueprints,
            mvps,
            gallery,
            emotes,
        )
    });
    let mods = paks.join("~mods");
    let exists = |m: &str| swap::mod_files(&mods, m).iter().any(|f| f.exists());
    let emote_mods = std::fs::read_dir(&mods)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(swap::EMOTE_TABLE_MOD_PREFIX)
        });
    if blueprints.is_empty()
        && mvps.is_empty()
        && emotes.is_empty()
        && !exists(swap::SKIN_TABLE_MOD)
        && !exists(swap::MVP_TABLE_MOD)
        && !exists(swap::GALLERY_TABLE_MOD)
        && !exists(swap::EMOTE_UI_TABLE_MOD)
        && !emote_mods
    {
        return;
    }
    h.log("  Updating the skin, MVP and emote tables...".into());
    let res = Game::open(&paks).and_then(|game| {
        let work = cache_dir().join("table");
        Ok((
            swap::write_skin_table(&game, &work, &mods, &blueprints)?,
            swap::write_mvp_table(&game, &work, &mods, &mvps)?,
            swap::write_gallery_table(&game, &work, &mods, &gallery)?,
            swap::write_emote_tables(&game, &work, &mods, &emotes)?,
        ))
    });
    release_memory();
    match res {
        Ok((b, m, _, e)) => h.log(format!(
            "  Tables: {b} skin blueprint(s), {m} MVP swap(s), {e} emote swap(s) in use"
        )),
        Err(e) => h.log(format!("[ERROR] Table update failed: {e:#}")),
    }
}

/// Forget a swap and delete its mod files from ~mods.
fn delete_swap(settings: &mut Settings, skin_id: &str, log: &mut Vec<String>) {
    if let Some(rec) = settings.active_swaps.remove(skin_id) {
        delete_record(&settings.game_paks_dir, &rec, log);
    }
}

/// Display name of an emote.
fn emote_label(emote: &Emote) -> String {
    emote.name.clone()
}

/// Display name of an MVP: the skin it belongs to and which MVP it is.
fn mvp_label(c: &Character, mvp: &Mvp) -> String {
    let skin = c
        .skins
        .iter()
        .find(|s| s.id == mvp.id[..7])
        .map_or_else(|| format!("Skin {}", &mvp.id[..7]), |s| s.name.clone());
    format!("{skin} MVP")
}

fn delete_record(paks_dir: &str, rec: &SwapRecord, log: &mut Vec<String>) {
    let mods_dir = Path::new(paks_dir).join("~mods");
    let recorded = [&rec.pak_path, &rec.utoc_path, &rec.ucas_path].map(PathBuf::from);
    for f in recorded
        .into_iter()
        .chain(swap::mod_files(&mods_dir, &rec.mod_name))
    {
        if f.is_file()
            && let Err(e) = std::fs::remove_file(&f)
        {
            log.push(format!("[WARN] Could not delete {}: {e}", f.display()));
        }
    }
    log.push(format!("Removed {}", rec.skin_name));
}

fn fonts(ctx: &egui::Context) {
    use egui::{FontData, FontFamily};
    let mut f = egui::FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        f.font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    };
    add(
        "barlow",
        include_bytes!("../assets/fonts/Barlow-Regular.ttf"),
    );
    add(
        "barlow-medium",
        include_bytes!("../assets/fonts/Barlow-Medium.ttf"),
    );
    add(
        "display",
        include_bytes!("../assets/fonts/BarlowCondensed-SemiBoldItalic.ttf"),
    );
    let fallback = f.families[&FontFamily::Proportional].clone();
    let family = |main: &str| [vec![main.to_string()], fallback.clone()].concat();
    f.families
        .insert(FontFamily::Proportional, family("barlow"));
    f.families
        .insert(FontFamily::Name("medium".into()), family("barlow-medium"));
    f.families
        .insert(FontFamily::Name("display".into()), family("display"));
    ctx.set_fonts(f);
}

fn font(family: &str, size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(family.into()))
}

fn style(ctx: &egui::Context) {
    use egui::{FontFamily, TextStyle};
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        s.text_styles = [
            (
                TextStyle::Small,
                egui::FontId::new(12.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Body,
                egui::FontId::new(15.0, FontFamily::Proportional),
            ),
            (TextStyle::Button, font("medium", 15.0)),
            (TextStyle::Heading, font("display", 30.0)),
            (
                TextStyle::Monospace,
                egui::FontId::new(13.0, FontFamily::Monospace),
            ),
        ]
        .into();
        let v = &mut s.visuals;
        v.panel_fill = VOID;
        v.window_fill = PANEL;
        v.window_stroke = Stroke::new(1.0, LINE);
        v.window_corner_radius = CornerRadius::same(4);
        v.extreme_bg_color = PANEL; // text fields
        v.override_text_color = Some(TEXT);
        v.hyperlink_color = YELLOW;
        v.selection.bg_fill = LINE;
        v.selection.stroke = Stroke::new(1.0, YELLOW);
        for w in [
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = CornerRadius::same(3);
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        v.widgets.inactive.weak_bg_fill = RAISED;
        v.widgets.inactive.bg_fill = RAISED;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
        v.widgets.hovered.weak_bg_fill = LINE;
        v.widgets.hovered.bg_fill = LINE;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, MUTED);
        v.widgets.active.weak_bg_fill = LINE;
        v.widgets.active.bg_stroke = Stroke::new(1.0, YELLOW);
        s.spacing.button_padding = Vec2::new(14.0, 7.0);
        s.spacing.item_spacing = Vec2::new(10.0, 10.0);
        s.spacing.interact_size.y = 32.0;
    });
}

// ---------------------------------------------------------------------------
// Shapes: the game's slanted tabs. Used only for the primary action, the
// selected hero and the swapped tag.
// ---------------------------------------------------------------------------

const SLANT: f32 = 9.0;

fn slanted(rect: egui::Rect, slant: f32) -> Vec<egui::Pos2> {
    vec![
        rect.left_bottom(),
        rect.left_top() + Vec2::new(slant, 0.0),
        rect.right_top(),
        rect.right_bottom() - Vec2::new(slant, 0.0),
    ]
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Primary,
    Danger,
    Ghost,
}

/// A slanted button. Keyboard-focusable, with a visible focus outline.
fn slant_button(
    ui: &mut egui::Ui,
    text: &str,
    tone: Tone,
    enabled: bool,
    width: f32,
) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.into(), font("medium", 15.0), TEXT);
    let size = Vec2::new(width.max(galley.size().x + 2.0 * SLANT + 24.0), 36.0);
    let (rect, resp) = ui.allocate_exact_size(
        size,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, text));
    if ui.is_rect_visible(rect) {
        let lit = enabled && resp.hovered();
        let (fill, stroke, ink) = match tone {
            Tone::Primary => (
                if lit {
                    Color32::from_rgb(0xff, 0xd6, 0x5c)
                } else {
                    YELLOW
                },
                Stroke::NONE,
                INK,
            ),
            Tone::Danger => (
                if lit {
                    Color32::from_rgb(0xf0, 0x48, 0x56)
                } else {
                    RED
                },
                Stroke::NONE,
                Color32::WHITE,
            ),
            Tone::Ghost => (
                if lit { RAISED } else { Color32::TRANSPARENT },
                Stroke::new(1.0, if lit { MUTED } else { LINE }),
                TEXT,
            ),
        };
        let (fill, ink) = if enabled {
            (fill, ink)
        } else {
            (fill.gamma_multiply(0.35), ink.gamma_multiply(0.6))
        };
        let rect = if resp.is_pointer_button_down_on() {
            rect.translate(Vec2::new(0.0, 1.0))
        } else {
            rect
        };
        ui.painter().add(egui::Shape::convex_polygon(
            slanted(rect, SLANT),
            fill,
            stroke,
        ));
        if resp.has_focus() {
            ui.painter().add(egui::Shape::closed_line(
                slanted(rect.expand(3.0), SLANT),
                Stroke::new(1.5, TEXT),
            ));
        }
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            font("medium", 15.0),
            ink,
        );
    }
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Image from the cache, or a quiet placeholder while it's missing.
fn paint_image(ui: &egui::Ui, path: &Path, rect: egui::Rect, dim: bool) {
    paint_image_uv(
        ui,
        path,
        rect,
        dim,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
    );
}

/// Same, showing only the `uv` part of the image (0..1 coordinates).
fn paint_image_uv(ui: &egui::Ui, path: &Path, rect: egui::Rect, dim: bool, uv: egui::Rect) {
    if path.exists() {
        egui::Image::new(format!("file://{}", path.display()))
            .uv(uv)
            .tint(if dim {
                Color32::from_gray(80)
            } else {
                Color32::WHITE
            })
            .paint_at(ui, rect);
    } else {
        ui.painter().rect_filled(rect, 0, RAISED);
    }
}

// ---------------------------------------------------------------------------
// Skin cards
// ---------------------------------------------------------------------------

const CARD_W: f32 = 176.0;
const CHAMFER: f32 = 16.0;

/// Lay items out in rows of as many cards as fit, stretched to fill the row.
/// `card` gets the card width.
fn rows<T>(ui: &mut egui::Ui, items: &[T], mut card: impl FnMut(&mut egui::Ui, &T, f32)) {
    let gap = 14.0;
    let avail = ui.available_width() - 2.0; // card stroke
    let cols = (((avail + gap) / (CARD_W + gap)) as usize).max(1);
    let width = ((avail - gap * (cols - 1) as f32) / cols as f32 - 2.0).floor();
    for row in items.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            row.iter().for_each(|it| card(ui, it, width))
        });
        ui.add_space(4.0);
    }
}

/// A skin card: full-bleed art with a chamfered corner, then `body`.
/// The card itself takes no clicks, so buttons inside keep theirs.
fn card(
    ui: &mut egui::Ui,
    width: f32,
    image: &Path,
    dim: bool,
    active: bool,
    body: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let resp = egui::Frame::new()
        .fill(PANEL)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 6.0;
                let (art, _) = ui.allocate_exact_size(Vec2::new(width, width), Sense::hover());
                paint_image(ui, image, art, dim);
                // Clip the top-left corner like the game's panels.
                let c = art.left_top();
                ui.painter().add(egui::Shape::convex_polygon(
                    vec![c, c + Vec2::new(CHAMFER, 0.0), c + Vec2::new(0.0, CHAMFER)],
                    VOID,
                    Stroke::NONE,
                ));
                if active {
                    let tag = egui::Rect::from_min_size(
                        art.left_top() + Vec2::new(CHAMFER - 4.0, 8.0),
                        Vec2::new(70.0, 22.0),
                    );
                    ui.painter().add(egui::Shape::convex_polygon(
                        slanted(tag, 6.0),
                        YELLOW,
                        Stroke::NONE,
                    ));
                    ui.painter().text(
                        tag.center(),
                        egui::Align2::CENTER_CENTER,
                        "Swapped",
                        font("medium", 13.0),
                        INK,
                    );
                }
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 10,
                        top: 2,
                        bottom: 10,
                    })
                    .show(ui, |ui| ui.vertical(body));
            })
        })
        .response;
    // Border follows the clipped corner.
    let r = resp.rect.shrink(if active { 1.0 } else { 0.5 });
    let c = CHAMFER - 0.5;
    ui.painter().add(egui::Shape::closed_line(
        vec![
            r.left_top() + Vec2::new(c, 0.0),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
            r.left_top() + Vec2::new(0.0, c),
        ],
        Stroke::new(
            if active { 2.0 } else { 1.0 },
            if active { YELLOW } else { LINE },
        ),
    ));
    resp
}

/// A card's note, in a fixed three-line area so every card in a row is the same
/// height and the buttons line up. Longer notes are clipped; the full note
/// shows on hover.
fn card_note(ui: &mut egui::Ui, note: &str, tip: &str) {
    const LINES: usize = 3;
    let width = ui.available_width();
    let galley = egui::WidgetText::from(RichText::new(note).small().color(MUTED)).into_galley(
        ui,
        Some(egui::TextWrapMode::Wrap),
        width,
        egui::TextStyle::Small,
    );
    let line = galley.rows.first().map_or(galley.size().y, |r| r.height());
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(width, line * LINES as f32), Sense::hover());
    ui.painter()
        .with_clip_rect(rect.intersect(ui.clip_rect()))
        .galley(rect.left_top(), galley, MUTED);
    if !tip.is_empty() {
        resp.on_hover_text(tip);
    }
}

// ---------------------------------------------------------------------------
// Frame
// ---------------------------------------------------------------------------

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        if self.h.with(|s| std::mem::take(&mut s.images_changed)) {
            ctx.forget_all_images();
        }
        let (busy, status, error, update) = {
            let s = self.h.shared.lock().unwrap();
            (s.busy, s.status.clone(), s.error.clone(), s.update.clone())
        };
        let characters = self.h.with(|s| s.characters.clone());

        self.top_bar(ui, busy, status.as_deref());
        if let Some((tag, url)) = &update {
            egui::Panel::top("update")
                .frame(
                    egui::Frame::new()
                        .fill(RAISED)
                        .inner_margin(egui::Margin::symmetric(20, 8)),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(format!("Rivals Swapper {tag} is out."));
                        ui.hyperlink_to("Download it", url);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Dismiss").clicked() {
                                self.h.with(|s| s.update = None);
                            }
                        });
                    });
                });
        }
        self.activity(ui);

        if error.is_none() && !characters.is_empty() {
            egui::Panel::left("roster")
                .resizable(false)
                .exact_size(264.0)
                .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin {
                    left: 14,
                    right: 10,
                    top: 16,
                    bottom: 10,
                }))
                .show(ui, |ui| self.roster(ui, &characters));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(VOID).inner_margin(egui::Margin {
                left: 32,
                right: 24,
                top: 22,
                bottom: 12,
            }))
            .show(ui, |ui| {
                if let Some(err) = error {
                    self.notice(
                        ui,
                        "Marvel Rivals wasn't found",
                        &err,
                        Some("Choose the Paks folder"),
                    );
                    return;
                }
                if characters.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() * 0.35);
                        ui.spinner();
                        ui.label(
                            RichText::new(
                                status
                                    .clone()
                                    .unwrap_or_else(|| "Reading the game's hero list…".into()),
                            )
                            .color(MUTED),
                        );
                    });
                    return;
                }
                let selected = self
                    .selected
                    .clone()
                    .and_then(|id| characters.iter().find(|c| c.char_id == id).cloned());
                match selected.or_else(|| characters.first().cloned()) {
                    Some(c) => {
                        self.selected = Some(c.char_id.clone());
                        self.stage(ui, &c, busy);
                    }
                    None => self.notice(
                        ui,
                        "No heroes found",
                        "The game files didn't list any heroes.",
                        None,
                    ),
                }
            });

        self.dialogs(ctx);
    }
}

impl App {
    fn top_bar(&mut self, ui: &mut egui::Ui, busy: bool, status: Option<&str>) {
        egui::Panel::top("top")
            .exact_size(58.0)
            .frame(
                egui::Frame::new()
                    .fill(VOID)
                    .inner_margin(egui::Margin::symmetric(20, 0))
                    .stroke(Stroke::new(1.0, LINE)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    // The logo, cropped to its artwork (the PNG is mostly padding).
                    let (mark, _) = ui.allocate_exact_size(Vec2::new(54.0, 31.0), Sense::hover());
                    egui::Image::new(egui::include_image!("../assets/RivalsIcon_NoOutline.png"))
                        .uv(egui::Rect::from_min_max(
                            egui::pos2(0.08, 0.26),
                            egui::pos2(0.92, 0.74),
                        ))
                        .paint_at(ui, mark);
                    ui.label(
                        RichText::new("Rivals Swapper")
                            .font(font("display", 26.0))
                            .color(TEXT),
                    );
                    ui.add_space(16.0);
                    if status.is_some() || busy {
                        ui.spinner();
                        ui.label(RichText::new(status.unwrap_or("Working…")).color(MUTED));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if slant_button(ui, "Launch game", Tone::Primary, true, 0.0)
                            .on_hover_text("Start Marvel Rivals through Steam")
                            .clicked()
                        {
                            self.launch();
                        }
                        ui.add_space(6.0);
                        if slant_button(ui, "Settings", Tone::Ghost, true, 0.0).clicked() {
                            self.open_settings();
                        }
                    });
                });
            });
    }

    fn open_settings(&mut self) {
        self.paks_input = self.h.with(|s| s.settings.game_paks_dir.clone());
        self.settings_open = true;
    }

    /// Latest log line in a strip at the bottom; the full log opens above it.
    fn activity(&mut self, ui: &mut egui::Ui) {
        let log = self.h.with(|s| s.log.clone());
        let color = |line: &str| {
            if line.contains("[ERROR]") {
                RED
            } else if line.contains("[WARN]") {
                YELLOW
            } else {
                MUTED
            }
        };
        egui::Panel::bottom("activity")
            .exact_size(36.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(20, 0))
                    .stroke(Stroke::new(1.0, LINE)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let last = log.last().map(|l| l.trim().to_string()).unwrap_or_else(|| {
                        "Pick a hero, then a skin to swap onto their default skin.".into()
                    });
                    let toggle = if self.log_open {
                        "Hide log"
                    } else {
                        "Show log"
                    };
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!log.is_empty(), egui::Button::new(toggle).frame(false))
                            .clicked()
                        {
                            self.log_open = !self.log_open;
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(&last).color(color(&last)))
                                    .truncate(),
                            );
                        });
                    });
                });
            });
        if self.log_open {
            egui::Panel::bottom("log")
                .resizable(true)
                .default_size(150.0)
                .frame(
                    egui::Frame::new()
                        .fill(PANEL)
                        .inner_margin(egui::Margin::symmetric(20, 10)),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Log").font(font("medium", 15.0)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Clear").clicked() {
                                self.h.with(|s| s.log.clear());
                                self.log_open = false;
                            }
                        });
                    });
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            for line in &log {
                                ui.label(RichText::new(line).monospace().color(color(line)));
                            }
                        });
                });
        }
    }

    /// Centered message with an optional way out.
    fn notice(&mut self, ui: &mut egui::Ui, title: &str, body: &str, action: Option<&str>) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.label(RichText::new(title).heading());
            ui.add_space(4.0);
            ui.label(RichText::new(body).color(MUTED));
            if let Some(action) = action {
                ui.add_space(12.0);
                if slant_button(ui, action, Tone::Primary, true, 0.0).clicked() {
                    self.open_settings();
                }
            }
        });
    }

    fn roster(&mut self, ui: &mut egui::Ui, characters: &[Character]) {
        let settings = self.h.with(|s| s.settings.clone());
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("Find a hero")
                .desired_width(f32::INFINITY)
                .margin(Vec2::new(10.0, 8.0)),
        );
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.swapped_only, false, "All heroes");
            let n = settings.swap_count();
            ui.selectable_value(&mut self.swapped_only, true, format!("Swapped ({n})"));
        });
        ui.add_space(6.0);
        let query = self.search.to_lowercase();
        let shown: Vec<&Character> = characters
            .iter()
            .filter(|c| {
                c.name.to_lowercase().contains(&query)
                    && (!self.swapped_only || settings.is_swapped(&c.char_id))
            })
            .collect();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                if shown.is_empty() {
                    ui.label(
                        RichText::new(if self.swapped_only {
                            "Nothing swapped yet."
                        } else {
                            "No hero matches that name."
                        })
                        .color(MUTED),
                    );
                }
                for c in shown {
                    let selected = self.selected.as_deref() == Some(c.char_id.as_str());
                    let swapped = settings.is_swapped(&c.char_id);
                    if roster_row(ui, c, selected, swapped).clicked() {
                        self.selected = Some(c.char_id.clone());
                    }
                }
            });
    }

    /// The selected hero: name, what's swapped, and their skins.
    fn stage(&mut self, ui: &mut egui::Ui, c: &Character, busy: bool) {
        let settings = self.h.with(|s| s.settings.clone());
        let current = settings.swaps_for(&c.char_id).next().cloned();
        let mut action: Option<Result<Skin, String>> = None; // Ok = swap, Err(id) = remove

        ui.label(
            RichText::new(&c.name)
                .font(font("display", 46.0))
                .color(TEXT),
        );
        ui.horizontal(|ui| match &current {
            Some(rec) => {
                ui.label(
                    RichText::new(format!(
                        "Wearing {}. Equip {}'s default skin in-game to see it.",
                        rec.skin_name, c.name
                    ))
                    .color(TEXT),
                );
                if slant_button(ui, "Remove swap", Tone::Ghost, !busy, 0.0).clicked() {
                    action = Some(Err(rec.skin_id.clone()));
                }
            }
            None => {
                let n = c.alt_skins().filter(|s| s.swappable).count();
                ui.label(
                    RichText::new(format!("Default skin. {n} skins can be swapped onto it."))
                        .color(MUTED),
                );
            }
        });
        if c.texture_risk {
            ui.add_space(8.0);
            texture_warning(ui, &c.name);
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, Tab::Skins, "Skins");
            let mvps = c.mvps.iter().filter(|m| m.is_special()).count();
            ui.add_enabled_ui(mvps > 0, |ui| {
                ui.selectable_value(&mut self.tab, Tab::Mvp, format!("MVP ({mvps})"));
            });
            let emotes = c.emotes.iter().filter(|e| e.is_choice()).count();
            ui.add_enabled_ui(emotes > 0, |ui| {
                ui.selectable_value(&mut self.tab, Tab::Emotes, format!("Emotes ({emotes})"));
            });
        });
        ui.add_space(8.0);
        match self.tab {
            Tab::Mvp if c.mvps.iter().any(Mvp::is_special) => {
                return self.mvp_view(ui, c, busy, &settings);
            }
            Tab::Emotes if c.emotes.iter().any(Emote::is_choice) => {
                return self.emote_view(ui, c, busy, &settings);
            }
            _ => {}
        }

        let skins: Vec<&Skin> = c.alt_skins().collect();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            rows(ui, &skins, |ui, skin, width| {
                let active = settings.active_swaps.contains_key(&skin.id);
                card(ui, width, &images::skin(&skin.id), !skin.swappable, active, |ui| {
                    ui.add(egui::Label::new(RichText::new(&skin.name).font(font("medium", 15.5))).truncate()).on_hover_text(&skin.name);
                    let (note, tip) = if !skin.swappable {
                        ("Not in the game files", "The game files have no model for this skin, so it can't be swapped.")
                    } else if skin.retexture {
                        ("Recolor", "Swapped as the skin it recolors, with this skin's materials.")
                    } else {
                        ("Own model", "This skin has its own model.")
                    };
let (mut note, mut tip) = (note.to_string(), tip.to_string());
                    if skin.custom_sounds && skin.swappable {
                        note += ", custom sounds";
                        tip += " Its ability sound effects can be swapped too.";
                    }
                    if skin.custom_effects && skin.swappable {
                        note += ", custom effects";
                        tip += " Its ability VFX can be swapped too.";
                    }
                    if c.own_mvp(&skin.id).is_some() && skin.swappable {
                        note += ", own MVP";
                        tip += " Its own MVP can replace the default one.";
                    }

                    card_note(ui, &note, &tip);
                    ui.add_space(2.0);
                    let w = ui.available_width();
                    if active {
                        if slant_button(ui, "Remove", Tone::Danger, !busy, w).clicked() {
                            action = Some(Err(skin.id.clone()));
                        }
                    } else if skin.swappable {
                        let label = if current.is_some() { "Swap instead" } else { "Swap" };
                        if slant_button(ui, label, Tone::Primary, !busy, w).clicked() {
                            action = Some(Ok((*skin).clone()));
                        }
                    } else {
                        slant_button(ui, "Unavailable", Tone::Ghost, false, w);
                    }
                });
            });
        });
        match action {
            // Skins with extras to bring along ask first; the rest swap straight away.
            Some(Ok(skin))
                if skin.custom_sounds || skin.custom_effects || c.own_mvp(&skin.id).is_some() =>
            {
                let options = swap::SwapOptions {
                    sounds: true,
                    effects: true,
                    mvp: true,
                };
                self.swap_prompt = Some((c.clone(), skin, options));
            }
            Some(Ok(skin)) => {
                let options = swap::SwapOptions {
                    sounds: true,
                    effects: true,
                    mvp: true,
                };
                self.start_swap(c.clone(), skin, options)
            }
            Some(Err(id)) => self.remove_swaps(vec![id]),
            None => {}
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some((character, skin, options)) = &mut self.swap_prompt {
            let mut close = None;
            let modal = egui::Modal::new(egui::Id::unique("swap_options"))
                .frame(dialog_frame())
                .show(ctx, |ui| {
                    ui.set_width(380.0);
                    ui.label(RichText::new(format!("Swap {}", skin.name)).heading());
                    ui.label(
                        RichText::new(format!(
                            "{}'s default skin becomes {}. Choose what comes along with the model.",
                            character.name, skin.name
                        ))
                        .color(MUTED),
                    );
                    ui.add_space(8.0);
                    if skin.custom_sounds {
                        ui.checkbox(&mut options.sounds, "Sound effects").on_hover_text(
                            "This skin's own ability sound effects replace the default ones.",
                        );
                    }
                    if skin.custom_effects {
                        ui.checkbox(&mut options.effects, "Visual effects").on_hover_text(
                            "This skin's own ability VFX, body effects and effect colors replace the default ones.",
                        );
                    }
                    if character.own_mvp(&skin.id).is_some() {
                        ui.checkbox(&mut options.mvp, "Its own MVP").on_hover_text(
                            "The default MVP plays this skin's own MVP, whole scene and stage included. Unticked, the default MVP plays with this skin.",
                        );
                    }
                    if skin.missing_effects {
                        ui.label(
                            RichText::new("Some of this skin's effects can't be swapped and will stay default.")
                                .small()
                                .color(YELLOW),
                        );
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if slant_button(ui, "Swap", Tone::Primary, true, 0.0).clicked() {
                            close = Some(true);
                        }
                        if slant_button(ui, "Cancel", Tone::Ghost, true, 0.0).clicked() {
                            close = Some(false);
                        }
                    });
                });
            // Escape or a click outside cancels.
            if close.is_none() && modal.should_close() {
                close = Some(false);
            }
            match close {
                Some(true) => {
                    let (character, skin, options) = self.swap_prompt.take().unwrap();
                    self.start_swap(character, skin, options);
                }
                Some(false) => self.swap_prompt = None,
                None => {}
            }
        }

        if self.confirm_remove_all {
            let count = self.h.with(|s| s.settings.swap_count());
            egui::Modal::new(egui::Id::unique("remove_all")).frame(dialog_frame()).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new("Remove every swap?").heading());
                ui.label(RichText::new(format!("This deletes the {count} swap mod(s) from ~mods. Heroes go back to their default look.")).color(MUTED));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if slant_button(ui, "Remove all", Tone::Danger, true, 0.0).clicked() {
                        // MVP choices go first; removing the skin swaps rewrites the tables.
                        let ids = self.h.with(|s| {
                            s.settings.mvp_swaps.clear();
                            s.settings.emote_swaps.clear();
                            s.settings.active_swaps.keys().cloned().collect()
                        });
                        self.log_open = true;
                        self.remove_swaps(ids);
                        self.confirm_remove_all = false;
                    }
                    if slant_button(ui, "Keep them", Tone::Ghost, true, 0.0).clicked() {
                        self.confirm_remove_all = false;
                    }
                });
            });
        }

        if self.settings_open {
            egui::Modal::new(egui::Id::unique("settings")).frame(dialog_frame()).show(ctx, |ui| {
                ui.set_width(560.0);
                ui.label(RichText::new("Settings").heading());
                ui.add_space(6.0);
                ui.label(RichText::new("Game Paks folder").font(font("medium", 15.0)));
                ui.label(RichText::new("…/MarvelRivals/MarvelGame/Marvel/Content/Paks").small().color(MUTED));
                ui.add(egui::TextEdit::singleline(&mut self.paks_input).desired_width(f32::INFINITY).margin(Vec2::new(10.0, 8.0)));
                let valid = is_paks_dir(Path::new(&self.paks_input));
                ui.horizontal(|ui| {
                    if ui.button("Browse…").clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder()
                    {
                        self.paks_input = dir.to_string_lossy().into_owned();
                    }
                    if ui.button("Find it for me").clicked() {
                        match find_game_paks() {
                            Some(p) => self.paks_input = p.to_string_lossy().into_owned(),
                            None => self.h.log("[WARN] Marvel Rivals isn't in any Steam library found on this computer.".into()),
                        }
                    }
                    if !self.paks_input.is_empty() && !valid {
                        ui.label(RichText::new("This folder has no game files (.utoc).").color(RED));
                    }
                });
                ui.add_space(8.0);
                let has_swaps = self.h.with(|s| s.settings.swap_count() > 0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Swaps").font(font("medium", 15.0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(has_swaps, egui::Button::new(RichText::new("Remove every swap").color(if has_swaps { RED } else { MUTED }))).clicked() {
                            self.settings_open = false;
                            self.confirm_remove_all = true;
                        }
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if slant_button(ui, "Save", Tone::Primary, valid, 0.0).clicked() {
                        let changed = self.h.with(|s| {
                            let changed = s.settings.game_paks_dir != self.paks_input || s.error.is_some();
                            s.settings.game_paks_dir = self.paks_input.clone();
                            let _ = s.settings.save();
                            if changed {
                                s.characters.clear();
                                s.error = None;
                            }
                            changed
                        });
                        if changed {
                            self.h.load();
                        }
                        self.settings_open = false;
                    }
                    if slant_button(ui, "Cancel", Tone::Ghost, true, 0.0).clicked() {
                        self.settings_open = false;
                    }
                });
            });
        }
    }
}

fn texture_risk_text(hero: &str) -> String {
    format!(
        "{hero}'s game logic puts the default skin's texture on part of the model, so a swap may show it on the wrong part. \
         Depending on the skin it can be very noticeable, minor and easy to miss, or not show up at all."
    )
}

/// A yellow-edged note under the hero's name, for heroes whose swaps can show
/// the default skin's texture on part of the model.
fn texture_warning(ui: &mut egui::Ui, hero: &str) {
    let resp = egui::Frame::new()
        .fill(RAISED)
        .stroke(Stroke::new(1.0, LINE))
        .inner_margin(egui::Margin {
            left: 18,
            right: 14,
            top: 10,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.set_max_width(760.0);
            ui.label(
                RichText::new("Textures may be off on this hero")
                    .font(font("medium", 15.0))
                    .color(YELLOW),
            );
            ui.label(RichText::new(texture_risk_text(hero)).color(TEXT));
        })
        .response;
    // Yellow edge on the left marks it as a caution, not an error.
    let r = resp.rect;
    ui.painter().rect_filled(
        egui::Rect::from_min_max(r.left_top(), egui::pos2(r.left() + 4.0, r.bottom())),
        0,
        YELLOW,
    );
}

fn dialog_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(4)
        .inner_margin(24)
}

/// One hero in the roster: portrait, name, and a yellow tick when swapped.
/// The selected hero sits on a slanted yellow plate.
fn roster_row(ui: &mut egui::Ui, c: &Character, selected: bool, swapped: bool) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, &c.name));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if selected {
            p.add(egui::Shape::convex_polygon(
                slanted(rect.shrink2(Vec2::new(0.0, 2.0)), 12.0),
                YELLOW,
                Stroke::NONE,
            ));
        } else if resp.hovered() {
            p.rect_filled(rect.shrink2(Vec2::new(0.0, 2.0)), 2, RAISED);
        }
        if resp.has_focus() {
            p.rect_stroke(rect, 2, Stroke::new(1.5, TEXT), egui::StrokeKind::Inside);
        }
        let img =
            egui::Rect::from_min_size(rect.left_top() + Vec2::new(14.0, 5.0), Vec2::splat(36.0));
        // Head and shoulders of the full-body portrait.
        let face = egui::Rect::from_min_max(egui::pos2(0.3, 0.04), egui::pos2(0.7, 0.44));
        p.rect_filled(
            img,
            2,
            if selected {
                INK.gamma_multiply(0.15)
            } else {
                RAISED
            },
        );
        paint_image_uv(ui, &images::portrait(&c.char_id), img, false, face);
        let ink = if selected { INK } else { TEXT };
        p.text(
            img.right_center() + Vec2::new(12.0, 0.0),
            egui::Align2::LEFT_CENTER,
            &c.name,
            font("medium", 15.5),
            ink,
        );
        if swapped {
            let tick = egui::Rect::from_center_size(
                rect.right_center() - Vec2::new(22.0, 0.0),
                Vec2::new(14.0, 8.0),
            );
            p.add(egui::Shape::convex_polygon(
                slanted(tick, 4.0),
                if selected { INK } else { YELLOW },
                Stroke::NONE,
            ));
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cards with short and long notes put their buttons at the same height.
    #[test]
    fn card_buttons_line_up() {
        let ctx = egui::Context::default();
        fonts(&ctx);
        style(&ctx);
        let mut tops = vec![];
        let mut lines = 0;
        for _ in 0..2 {
            tops.clear();
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(900.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let notes = [
                        "Own model",
                        "Own model, custom sounds, custom effects, some VFX stay default",
                    ];
                    // The long note fits its three lines at the narrowest card.
                    lines = egui::WidgetText::from(RichText::new(notes[1]).small())
                        .into_galley(
                            ui,
                            Some(egui::TextWrapMode::Wrap),
                            CARD_W - 20.0,
                            egui::TextStyle::Small,
                        )
                        .rows
                        .len();
                    rows(ui, &notes, |ui, note, width| {
                        card(ui, width, Path::new("missing.png"), false, false, |ui| {
                            ui.label("Skin");
                            card_note(ui, note, "");
                            tops.push(
                                slant_button(ui, "Swap", Tone::Primary, true, ui.available_width())
                                    .rect
                                    .top(),
                            );
                        });
                    });
                },
            );
        }
        assert!(lines <= 3, "{lines} lines");
        assert_eq!(tops.len(), 2);
        assert_eq!(tops[0], tops[1], "{tops:?}");
    }

    /// A button inside a card must receive its own clicks.
    #[test]
    fn button_inside_card_clicks() {
        let ctx = egui::Context::default();
        // One frame; returns the button's rect and whether it was clicked.
        let frame = |events: Vec<egui::Event>| {
            let (mut rect, mut clicked) = (egui::Rect::NOTHING, false);
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::splat(600.0));
            let _ = ctx.run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| {
                    card(ui, CARD_W, Path::new("missing.png"), false, false, |ui| {
                        let r = slant_button(ui, "Swap", Tone::Primary, true, 0.0);
                        (rect, clicked) = (r.rect, r.clicked());
                    });
                },
            );
            (rect, clicked)
        };
        let pos = frame(vec![]).0.center();
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(vec![egui::Event::PointerMoved(pos)]);
        frame(vec![press(true)]);
        assert!(frame(vec![press(false)]).1);
    }
}
