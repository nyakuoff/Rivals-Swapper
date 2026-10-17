//! Skin icons and hero portraits: exported from the game with umodel (through
//! Wine off Windows), downscaled to PNG once, and re-exported after game updates.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use rayon::prelude::*;
use regex::Regex;

use crate::{command, tools_dir, walk};

const MAX_PX: u32 = 160;
const EXPORT_MASKS: [&str; 2] = ["img_skin_*", "img_heroportrait_*"];

pub fn dir() -> PathBuf {
    crate::cache_dir().join("images")
}
pub fn skin(skin_id: &str) -> PathBuf {
    dir().join(format!("skin_{skin_id}.png"))
}
pub fn portrait(char_id: &str) -> PathBuf {
    dir().join(format!("portrait_{char_id}.png"))
}

fn marker() -> PathBuf {
    dir().join(".fingerprint")
}

/// True when the images were exported for this exact game build.
pub fn up_to_date(fingerprint: &str) -> bool {
    std::fs::read_to_string(marker()).is_ok_and(|f| f == fingerprint)
}

fn wine_path(p: &Path) -> String {
    format!("Z:{}", p.display()).replace('/', "\\")
}

pub fn export(paks_dir: &Path, fingerprint: &str, status: &dyn Fn(String)) -> Result<()> {
    let exe = tools_dir().join("umodel/umodel_materials_ue5.exe");
    if !exe.exists() {
        bail!("umodel not found at {}", exe.display());
    }
    let out = crate::cache_dir().join("umodel_out");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out)?;
    let paks = link_game_paks(paks_dir)?;
    let res = run_umodel(&exe, &paks, &out, status);
    let _ = std::fs::remove_dir_all(&paks);
    res?;

    status("Processing images…".into());
    std::fs::create_dir_all(dir())?;
    let skin_re = Regex::new(r"(?i)img_skin_(\d{7})\.tga$")?;
    let portrait_re = Regex::new(r"(?i)img_heroportrait_(\d{4})\d{4}_portrait\.tga$")?;
    let tgas = walk(&out);
    tgas.par_iter().for_each(|tga| {
        let name = tga.file_name().unwrap_or_default().to_string_lossy();
        let dst = if let Some(m) = skin_re.captures(&name) {
            skin(&m[1])
        } else if let Some(m) = portrait_re.captures(&name) {
            portrait(&m[1])
        } else {
            return;
        };
        match image::open(tga) {
            Ok(img) => {
                if let Err(e) = img.thumbnail(MAX_PX, MAX_PX).save(&dst) {
                    eprintln!("[images] {}: {e}", dst.display());
                }
            }
            Err(e) => eprintln!("[images] {name}: {e}"),
        }
    });
    let _ = std::fs::remove_dir_all(&out);
    if tgas.is_empty() {
        bail!("umodel exported no images");
    }
    std::fs::write(marker(), fingerprint)?;
    Ok(())
}

/// umodel also scans ~mods and aborts on some mod containers (encrypted ones,
/// like Project Galacta's), so hand it a sibling folder of hard links to the game's own files.
/// Same volume as the game, so hard links always work.
fn link_game_paks(paks_dir: &Path) -> Result<PathBuf> {
    let dst = paks_dir
        .parent()
        .unwrap_or(paks_dir)
        .join(".rivals_swapper_paks");
    let _ = std::fs::remove_dir_all(&dst);
    std::fs::create_dir_all(&dst)?;
    for entry in std::fs::read_dir(paks_dir)?.flatten() {
        let src = entry.path();
        if src.is_file() {
            std::fs::hard_link(&src, dst.join(entry.file_name()))?;
        }
    }
    Ok(dst)
}

fn run_umodel(exe: &Path, paks: &Path, out: &Path, status: &dyn Fn(String)) -> Result<()> {
    let (path_arg, out_arg) = if cfg!(windows) {
        (paks.display().to_string(), out.display().to_string())
    } else {
        (wine_path(paks), wine_path(out))
    };

    for (i, mask) in EXPORT_MASKS.iter().enumerate() {
        status(format!(
            "Exporting textures [{}/{}]…",
            i + 1,
            EXPORT_MASKS.len()
        ));
        let mut cmd = if cfg!(windows) {
            command(exe)
        } else {
            command("wine")
        };
        if !cfg!(windows) {
            cmd.arg(exe);
        }
        let key = format!("-aes={}", crate::game::AES_KEY);
        let res = cmd
            .args([
                "-export",
                "-game=marv",
                &format!("-path={path_arg}"),
                &format!("-out={out_arg}"),
                &key,
                mask,
            ])
            .output();
        match res {
            Err(e) if !cfg!(windows) && e.kind() == std::io::ErrorKind::NotFound => {
                bail!("Wine isn't installed; it's needed to export skin images.")
            }
            Err(e) => bail!("couldn't run umodel: {e}"),
            Ok(_) => {}
        }
    }

    Ok(())
}
