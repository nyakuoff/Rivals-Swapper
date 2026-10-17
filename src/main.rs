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

use db::{Character, Skin};
use game::Game;
use settings::Settings;

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

    let icon =
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/RivalsIcon_NoOutline.png"))
            .unwrap_or_default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Rivals Swapper")
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([700.0, 480.0])
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native(
        "Rivals Swapper",
        options,
        Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))),
    )
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

const BG: Color32 = Color32::from_rgb(0x12, 0x12, 0x12);
const CARD: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x1e);
const BORDER: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
const ACCENT: Color32 = Color32::from_rgb(0xf0, 0xc2, 0x32);
const GREEN: Color32 = Color32::from_rgb(0x00, 0xb8, 0x94);
const RED: Color32 = Color32::from_rgb(0xd6, 0x30, 0x31);
const TEXT: Color32 = Color32::from_rgb(0xe0, 0xe0, 0xe0);
const DIM: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);

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

#[derive(Clone, PartialEq)]
enum View {
    Heroes,
    Skins(String), // char_id
}

struct App {
    h: Handle,
    view: View,
    search: String,
    swapped_only: bool,
    log_open: bool,
    settings_open: bool,
    paks_input: String,
    confirm_remove_all: bool,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        egui_extras::install_image_loaders(ctx);
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
            view: View::Heroes,
            search: String::new(),
            swapped_only: false,
            log_open: false,
            settings_open: false,
            paks_input: String::new(),
            confirm_remove_all: false,
        }
    }

    fn start_swap(&mut self, character: Character, skin: Skin) {
        self.log_open = true;
        self.h.with(|s| s.log.clear());
        self.h.run(move |h| {
            h.log(format!(
                "Swapping {} -> {} ({})",
                character.name, skin.name, skin.id
            ));
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
                Err(e) => h.log(format!("[ERROR] Swap failed: {e:#}")),
            }
        });
    }

    fn remove_swaps(&mut self, skin_ids: Vec<String>) {
        self.h.with(|s| {
            for id in skin_ids {
                delete_swap(&mut s.settings, &id, &mut s.log);
            }
            let _ = s.settings.save();
        });
    }

    fn launch(&mut self) {
        self.log_open = true;
        self.h.log("Launching Marvel Rivals...".into());
        if let Err(e) = open::that(STEAM_URL) {
            self.h.log(format!("[ERROR] Couldn't open Steam: {e}"));
        }
    }
}

/// Forget a swap and delete its mod files from ~mods.
fn delete_swap(settings: &mut Settings, skin_id: &str, log: &mut Vec<String>) {
    let Some(rec) = settings.active_swaps.remove(skin_id) else {
        return;
    };
    let mods_dir = Path::new(&settings.game_paks_dir).join("~mods");
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

fn style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        s.visuals.panel_fill = BG;
        s.visuals.window_fill = CARD;
        s.visuals.extreme_bg_color = Color32::from_rgb(0x18, 0x18, 0x18);
        s.visuals.override_text_color = Some(TEXT);
        s.visuals.selection.bg_fill = ACCENT.linear_multiply(0.5);
        s.visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
        s.visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
        s.visuals.widgets.active.corner_radius = CornerRadius::same(6);
        s.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.spacing.item_spacing = Vec2::new(10.0, 10.0);
    });
}

/// Image from the cache, or a flat placeholder while it doesn't exist.
fn thumb(ui: &mut egui::Ui, path: &Path, size: f32, dim: bool) {
    let tint = if dim {
        Color32::from_gray(90)
    } else {
        Color32::WHITE
    };
    if path.exists() {
        ui.add(
            egui::Image::new(format!("file://{}", path.display()))
                .fit_to_exact_size(Vec2::splat(size))
                .corner_radius(6)
                .tint(tint),
        );
    } else {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        ui.painter()
            .rect_filled(rect, 6, Color32::from_rgb(0x26, 0x26, 0x26));
    }
}

/// Lay items out in rows of as many `card_w`-wide cards as fit.
fn rows<T>(ui: &mut egui::Ui, items: &[T], mut card: impl FnMut(&mut egui::Ui, &T)) {
    const OUTER: f32 = CARD_W + 23.0; // inner margin + stroke
    let gap = ui.spacing().item_spacing.x;
    let cols = (((ui.available_width() + gap) / (OUTER + gap)) as usize).max(1);
    for row in items.chunks(cols) {
        ui.horizontal(|ui| row.iter().for_each(|it| card(ui, it)));
    }
}

const CARD_W: f32 = 150.0;

/// A card; `body` draws the contents. Not clickable itself, so buttons inside
/// keep their clicks (see `clickable` for whole-card clicks).
fn card(
    ui: &mut egui::Ui,
    width: f32,
    border: Color32,
    body: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.5, border))
        .corner_radius(10)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(width);
            ui.vertical_centered(body);
        })
        .response
}

/// Make a whole card clickable, with a hover outline. Only for cards without
/// buttons: the click area sits on top of the card's contents.
fn clickable(ui: &mut egui::Ui, card: egui::Response) -> egui::Response {
    let resp = card.interact(Sense::click());
    if resp.hovered() {
        ui.painter().rect_stroke(
            resp.rect,
            10,
            Stroke::new(1.5, ACCENT.linear_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

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

        egui::Panel::top("top")
            .frame(
                egui::Frame::new()
                    .fill(CARD)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Rivals Swapper")
                            .size(22.0)
                            .strong()
                            .color(ACCENT),
                    );
                    if let Some(st) = &status {
                        ui.add_space(12.0);
                        ui.spinner();
                        ui.label(RichText::new(st).color(DIM));
                    } else if busy {
                        ui.add_space(12.0);
                        ui.spinner();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Settings").clicked() {
                            self.paks_input = self.h.with(|s| s.settings.game_paks_dir.clone());
                            self.settings_open = true;
                        }
                        if ui.selectable_label(self.log_open, "Log").clicked() {
                            self.log_open = !self.log_open;
                        }
                        let has_swaps = self.h.with(|s| !s.settings.active_swaps.is_empty());
                        if ui
                            .add_enabled(!busy && has_swaps, egui::Button::new("Remove All"))
                            .clicked()
                        {
                            self.confirm_remove_all = true;
                        }
                        if ui
                            .add_enabled(!busy, egui::Button::new("Launch"))
                            .on_hover_text("Start Marvel Rivals")
                            .clicked()
                        {
                            self.launch();
                        }
                    });
                });
            });

        if let Some((tag, url)) = &update {
            egui::Panel::top("update")
                .frame(
                    egui::Frame::new()
                        .fill(Color32::from_rgb(0x1a, 0x2a, 0x1a))
                        .inner_margin(8),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(format!("Rivals Swapper {tag} is available."));
                        ui.hyperlink_to("Download", url);
                        if ui.small_button("✕").clicked() {
                            self.h.with(|s| s.update = None);
                        }
                    });
                });
        }

        if self.log_open {
            egui::Panel::bottom("log")
                .resizable(true)
                .default_size(160.0)
                .frame(egui::Frame::new().fill(CARD).inner_margin(10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Log").strong().color(DIM));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("✕").clicked() {
                                self.log_open = false;
                            }
                            if ui.small_button("Clear").clicked() {
                                self.h.with(|s| s.log.clear());
                            }
                        });
                    });
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            for line in &self.h.shared.lock().unwrap().log {
                                let color = if line.contains("[ERROR]") {
                                    RED
                                } else if line.contains("[WARN]") {
                                    ACCENT
                                } else {
                                    DIM
                                };
                                ui.label(RichText::new(line).monospace().color(color));
                            }
                        });
                });
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(20, 14)),
            )
            .show(ui, |ui| {
                if let Some(err) = error {
                    ui.vertical_centered(|ui| {
                        ui.add_space(120.0);
                        ui.label(RichText::new(err).size(16.0));
                        if ui.button("Open Settings").clicked() {
                            self.paks_input = self.h.with(|s| s.settings.game_paks_dir.clone());
                            self.settings_open = true;
                        }
                    });
                    return;
                }
                let characters = self.h.with(|s| s.characters.clone());
                if characters.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(160.0);
                        ui.spinner();
                        ui.label(
                            RichText::new(status.unwrap_or_else(|| "Loading…".into())).color(DIM),
                        );
                    });
                    return;
                }
                match self.view.clone() {
                    View::Heroes => self.hero_grid(ui, &characters),
                    View::Skins(id) => match characters.iter().find(|c| c.char_id == id) {
                        Some(c) => self.skin_grid(ui, c, busy),
                        None => self.view = View::Heroes,
                    },
                }
            });

        self.dialogs(ctx);
    }
}

impl App {
    fn hero_grid(&mut self, ui: &mut egui::Ui, characters: &[Character]) {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search heroes…")
                    .desired_width(260.0),
            );
            ui.checkbox(&mut self.swapped_only, "Swapped only");
        });
        ui.add_space(6.0);
        let settings = self.h.with(|s| s.settings.clone());
        let query = self.search.to_lowercase();
        let shown: Vec<&Character> = characters
            .iter()
            .filter(|c| {
                c.name.to_lowercase().contains(&query)
                    && (!self.swapped_only || settings.swaps_for(&c.char_id).next().is_some())
            })
            .collect();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                rows(ui, &shown, |ui, c| {
                    let swapped = settings.swaps_for(&c.char_id).count();
                    let border = if swapped > 0 { GREEN } else { BORDER };
                    let resp = card(ui, CARD_W, border, |ui| {
                        thumb(ui, &images::portrait(&c.char_id), 140.0, false);
                        ui.label(RichText::new(&c.name).strong());
                        let sub = if swapped > 0 {
                            RichText::new(format!("{swapped} swapped")).color(GREEN)
                        } else {
                            RichText::new(format!("{} skins", c.alt_skins().count())).color(DIM)
                        };
                        ui.label(sub.small());
                    });
                    if clickable(ui, resp).clicked() {
                        self.view = View::Skins(c.char_id.clone());
                    }
                });
            });
    }

    fn skin_grid(&mut self, ui: &mut egui::Ui, c: &Character, busy: bool) {
        ui.horizontal(|ui| {
            if ui.button("← Heroes").clicked() {
                self.view = View::Heroes;
            }
            ui.label(RichText::new(&c.name).size(20.0).strong());
        });
        ui.add_space(6.0);
        let settings = self.h.with(|s| s.settings.clone());
        let mut action: Option<Result<Skin, String>> = None; // Ok = swap, Err(id) = remove
        let skins: Vec<&Skin> = c.alt_skins().collect();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                rows(ui, &skins, |ui, skin| {
                    let active = settings.active_swaps.contains_key(&skin.id);
                    let border = if active { GREEN } else { BORDER };
                    card(ui, CARD_W, border, |ui| {
                        thumb(ui, &images::skin(&skin.id), 140.0, !skin.swappable);
                        ui.add(egui::Label::new(RichText::new(&skin.name).strong()).truncate())
                            .on_hover_text(&skin.name);
                        if skin.retexture {
                            ui.label(RichText::new("Recolor").small().color(DIM))
                                .on_hover_text(
                                    "Swapped as the skin it recolors, with this skin's materials.",
                                );
                        }
                        if active {
                            let b =
                                egui::Button::new(RichText::new("Remove").color(Color32::WHITE))
                                    .fill(RED)
                                    .min_size(Vec2::new(120.0, 0.0));
                            if ui.add_enabled(!busy, b).clicked() {
                                action = Some(Err(skin.id.clone()));
                            }
                        } else if skin.swappable {
                            let b = egui::Button::new(RichText::new("Swap").color(Color32::BLACK))
                                .fill(ACCENT)
                                .min_size(Vec2::new(120.0, 0.0));
                            if ui.add_enabled(!busy, b).clicked() {
                                action = Some(Ok((*skin).clone()));
                            }
                        } else {
                            ui.label(RichText::new("Not swappable").small().color(DIM))
                                .on_hover_text(
                                    "The game files have no mesh or skin blueprint for this skin.",
                                );
                        }
                    });
                });
            });
        match action {
            Some(Ok(skin)) => self.start_swap(c.clone(), skin),
            Some(Err(id)) => self.remove_swaps(vec![id]),
            None => {}
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.confirm_remove_all {
            let count = self.h.with(|s| s.settings.active_swaps.len());
            egui::Modal::new(egui::Id::unique("remove_all")).show(ctx, |ui| {
                ui.label(format!(
                    "Remove all {count} active swap(s) and delete their mod files?"
                ));
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Remove all").color(RED)).clicked() {
                        let ids = self
                            .h
                            .with(|s| s.settings.active_swaps.keys().cloned().collect());
                        self.log_open = true;
                        self.remove_swaps(ids);
                        self.view = View::Heroes;
                        self.confirm_remove_all = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_remove_all = false;
                    }
                });
            });
        }

        if self.settings_open {
            egui::Modal::new(egui::Id::unique("settings")).show(ctx, |ui| {
                ui.set_width(560.0);
                ui.heading("Settings");
                ui.label(
                    RichText::new(
                        "Game Paks folder (…/MarvelRivals/MarvelGame/Marvel/Content/Paks)",
                    )
                    .color(DIM),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.paks_input).desired_width(f32::INFINITY),
                );
                let valid = is_paks_dir(Path::new(&self.paks_input));
                ui.horizontal(|ui| {
                    if ui.button("Browse…").clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder()
                    {
                        self.paks_input = dir.to_string_lossy().into_owned();
                    }
                    if ui.button("Auto-detect").clicked() {
                        match find_game_paks() {
                            Some(p) => self.paks_input = p.to_string_lossy().into_owned(),
                            None => self.h.log(
                                "[WARN] Couldn't find Marvel Rivals in your Steam libraries."
                                    .into(),
                            ),
                        }
                    }
                    if !self.paks_input.is_empty() && !valid {
                        ui.label(RichText::new("No .utoc files in this folder").color(RED));
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.add_enabled(valid, egui::Button::new("Save")).clicked() {
                        let changed = self.h.with(|s| {
                            let changed =
                                s.settings.game_paks_dir != self.paks_input || s.error.is_some();
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
                    if ui.button("Cancel").clicked() {
                        self.settings_open = false;
                    }
                });
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                    card(ui, CARD_W, BORDER, |ui| {
                        let r = ui.button("Swap");
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
