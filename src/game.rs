//! Read-only access to the game's containers: IoStore (.utoc/.ucas) through
//! retoc and classic .pak through repak. Opened once per session.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, bail};
use retoc::container_header::{EIoContainerHeaderVersion, StoreEntry};
use retoc::iostore::{self, ChunkInfo, IoStoreTrait, PackageInfo};
use retoc::ser::ReadExt;
use retoc::{
    AesKey, Config, EIoChunkType, EIoStoreTocVersion, FGuid, FIoChunkId, FIoChunkIdRaw, FPackageId,
};

pub const AES_KEY: &str = "0x0C263D8C22DCB085894899C3A3796383E9BF9DE0CBFB08C9BF2DEF2E84F29D74";
/// Containers keyed with a non-default encryption key GUID: (guid as stored in the .utoc, key).
const EXTRA_AES_KEYS: &[(&str, &str)] = &[(
    "b53e535f8feeea44aacc46c9d5a1e707",
    "0xc1149b1dbecf933c290c328a764427c1ca2aec6d9efebd6a7750ee3fab0a059e",
)];

pub struct Game {
    paks_dir: PathBuf,
    store: Store,
    skins: OnceLock<SkinIndex>,
}

/// What the game files hold per skin id.
#[derive(Default)]
pub struct SkinIndex {
    /// Skeletal meshes as (form, package). Single-form heroes have one at
    /// Meshes/SK_{hero}_{skin} (a few skins use Mesh/); multi-form heroes (Hulk, Cloak & Dagger, ...) have
    /// one per form folder: Meshes/10111/SK_10111_1011001 is form 1.
    pub meshes: HashMap<String, Vec<(u8, String)>>,
    /// Skins with a {skin}_ChildBP (retextures have one but no mesh).
    pub blueprints: HashSet<String>,
}

impl Game {
    pub fn open(paks_dir: &Path) -> Result<Self> {
        let mut config = Config::default();
        config.aes_keys.insert(FGuid::default(), AES_KEY.parse()?);
        for (guid, key) in EXTRA_AES_KEYS {
            let bytes = hex::decode(guid)?;
            config
                .aes_keys
                .insert(Cursor::new(bytes).de()?, key.parse::<AesKey>()?);
        }
        let config = Arc::new(config);

        let mut store = Store::default();
        for entry in std::fs::read_dir(paks_dir).context("reading Paks folder")? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("utoc") {
                // A container for an unreleased patch can be on disk with a key we don't
                // have yet; skip it instead of failing the whole directory.
                match iostore::open(&path, config.clone()) {
                    Ok(c) => store.containers.push(c),
                    Err(e) => eprintln!("[game] skipping {}: {e:#}", path.display()),
                }
            }
        }
        if store.containers.is_empty() {
            bail!("No readable game containers in {}", paks_dir.display());
        }
        store.containers.sort_by(|a, b| {
            container_priority(b.container_name()).cmp(&container_priority(a.container_name()))
        });
        Ok(Self {
            paks_dir: paks_dir.to_path_buf(),
            store,
            skins: OnceLock::new(),
        })
    }

    /// Read a file from the classic .pak files, e.g. "Marvel/Content/Localization/Game/en/Game.locres".
    /// Patch paks are checked first; only the paks needed are opened.
    pub fn read_pak_file(&self, path: &str) -> Result<Vec<u8>> {
        let mut paks: Vec<PathBuf> = std::fs::read_dir(&self.paks_dir)?
            .filter_map(|e| Some(e.ok()?.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "pak"))
            .collect();
        let name = |p: &PathBuf| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        };
        paks.sort_by_key(|p| {
            (
                !name(p).ends_with("_P.pak"),
                !name(p).contains("Locres"),
                name(p),
            )
        });
        let key = pak_key()?;
        for pak_path in paks {
            let mut f = BufReader::new(File::open(&pak_path)?);
            let Ok(pak) = quiet_stdout(|| repak::PakBuilder::new().key(key.clone()).reader(&mut f))
            else {
                continue;
            };
            let rel = path
                .strip_prefix(pak.mount_point().trim_start_matches("../../../"))
                .unwrap_or(path);
            if let Ok(data) = pak.get(rel, &mut f) {
                return Ok(data);
            }
        }
        bail!("{path} not found in any .pak")
    }

    pub fn skins(&self) -> &SkinIndex {
        self.skins.get_or_init(|| {
            let mesh_re = regex::Regex::new(r"Marvel/Content/(Marvel/Characters/(\d{4})/(\d{7})/Mesh(?:es)?/(?:(\d{4,5})/)?SK_\d{4,5}_(\d{7}))\.uasset$").unwrap();
            let bp_re = regex::Regex::new(r"/Characters/\d{4}/(\d{7})/(\d{7})_ChildBP\.uasset$").unwrap();
            let mut index = SkinIndex::default();
            for c in self.store.chunks().filter(|c| c.id().get_chunk_type() == EIoChunkType::ExportBundleData) {
                let Some(path) = c.path() else { continue };
                if let Some(m) = bp_re.captures(&path) {
                    if m[1] == m[2] {
                        index.blueprints.insert(m[1].to_string());
                    }
                    continue;
                }
                let Some(m) = mesh_re.captures(&path) else { continue };
                let (hero, skin) = (&m[2], &m[3]);
                if !skin.starts_with(hero) {
                    continue;
                }
                let form = match m.get(4).map(|f| f.as_str()) {
                    None => Some(0),
                    Some(f) if f.len() == 5 && f.starts_with(hero) => f[4..].parse().ok(),
                    // Some heroes number form folders as hero ids: 1011, 1012, 1013.
                    Some(f) => f.parse::<i32>().ok().and_then(|f| u8::try_from(f - hero.parse::<i32>().ok()?).ok()).filter(|f| *f < 10),
                };
                let Some(form) = form else { continue };
                let package = format!("/Game/{}", &m[1]);
                let list = index.meshes.entry(skin.to_string()).or_default();
                // Prefer the mesh named after the skin when a form has several.
                match list.iter_mut().find(|(f, _)| *f == form) {
                    Some(slot) if m[5] == *skin && !slot.1.ends_with(skin) => slot.1 = package,
                    Some(_) => {}
                    None => list.push((form, package)),
                }
            }
            for list in index.meshes.values_mut() {
                list.sort();
            }
            index
        })
    }

    /// Convert one package to legacy .uasset/.uexp/.ubulk under `out_dir`, mirroring
    /// its pak path (Marvel/Content/...). Returns the written .uasset path.
    pub fn extract_package(&self, package: &str, out_dir: &Path) -> Result<PathBuf> {
        use retoc::asset_conversion::{FZenPackageContext, build_legacy};
        let id = package_id(package);
        let chunk_path = self
            .store
            .chunk_path(bundle_chunk(id))
            .with_context(|| format!("{package} not found in game files"))?;
        let rel = chunk_path.trim_start_matches("../../../");
        // Only warnings and errors (retoc logs one info line per package).
        let log = retoc::logging::Log::new(
            retoc::logging::LogLevel::Warning,
            Arc::new(retoc::logging::StdoutLogBackend {}),
        );
        let ctx = FZenPackageContext::create(
            &self.store,
            None,
            &log,
            Some(retoc::build_verse_cell_store(&vec![])),
        );
        build_legacy(
            &ctx,
            id,
            retoc::UEPath::new(rel),
            &retoc::FSFileWriter::new(out_dir),
        )
        .with_context(|| format!("converting {package}"))?;
        Ok(out_dir.join(rel))
    }
}

/// Key for the classic .pak files. NetEase's AES reads the key and data as
/// little-endian words; repak-rivals swaps the data words, so swap the key here.
fn pak_key() -> Result<aes::Aes256> {
    use aes::cipher::KeyInit;
    let mut bytes = hex::decode(AES_KEY.trim_start_matches("0x"))?;
    bytes.chunks_mut(4).for_each(|w| w.reverse());
    Ok(aes::Aes256::new_from_slice(&bytes)?)
}

/// repak-rivals println!s every deleted entry while reading an index (over a
/// million lines for the patch pak), so mute stdout around it.
/// ponytail: process-wide fd swap; drop it once repak-rivals stops printing.
#[cfg(unix)]
fn quiet_stdout<R>(f: impl FnOnce() -> R) -> R {
    use std::io::Write;
    // SAFETY: plain fd juggling on stdout; restored before returning.
    unsafe {
        let saved = libc::dup(1);
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        libc::dup2(null, 1);
        libc::close(null);
        let r = f();
        let _ = std::io::stdout().flush();
        libc::dup2(saved, 1);
        libc::close(saved);
        r
    }
}
#[cfg(not(unix))] // release builds have no console on Windows
fn quiet_stdout<R>(f: impl FnOnce() -> R) -> R {
    f()
}

fn package_id(package: &str) -> FPackageId {
    let utf16: Vec<u8> = package
        .to_ascii_lowercase()
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    FPackageId(cityhasher::hash(utf16))
}

fn bundle_chunk(id: FPackageId) -> FIoChunkId {
    FIoChunkId::from_package_id(id, 0, EIoChunkType::ExportBundleData)
}

/// Same ordering as retoc's directory backend: global first, then patch (`_N_P`) over base.
fn container_priority(name: &str) -> (bool, u32, &str) {
    let mut base = name;
    let mut version = 0;
    if let Some(n) = base.strip_suffix("_P") {
        base = n;
        version = 1;
        if let Some((n, v)) = base.rsplit_once('_')
            && let Ok(v) = v.parse::<u32>()
        {
            base = n;
            version = v + 2;
        }
    }
    (name == "global", version, base)
}

/// retoc's multi-container backend, minus the all-or-nothing open.
#[derive(Default)]
struct Store {
    containers: Vec<Box<dyn IoStoreTrait>>,
}

impl IoStoreTrait for Store {
    fn container_name(&self) -> &str {
        "VIRTUAL"
    }
    fn container_file_version(&self) -> Option<EIoStoreTocVersion> {
        self.containers
            .first()
            .and_then(|c| c.container_file_version())
    }
    fn container_header_version(&self) -> Option<EIoContainerHeaderVersion> {
        self.containers
            .iter()
            .find_map(|c| c.container_header_version())
    }
    fn print_info(&self, depth: usize) {
        for c in &self.containers {
            c.print_info(depth);
        }
    }
    fn read(&self, mut chunk_id: FIoChunkId) -> Result<Vec<u8>> {
        if let Some(v) = self.container_file_version() {
            chunk_id = chunk_id.with_version(v);
        }
        self.containers
            .iter()
            .find(|c| c.has_chunk_id(chunk_id))
            .with_context(|| format!("{chunk_id:?} not found"))?
            .read(chunk_id)
    }
    fn read_raw(&self, id: FIoChunkIdRaw) -> Result<Vec<u8>> {
        self.containers
            .iter()
            .find(|c| c.has_chunk_id_raw(id))
            .with_context(|| format!("{id:?} not found"))?
            .read_raw(id)
    }
    fn has_chunk_id(&self, id: FIoChunkId) -> bool {
        self.containers.iter().any(|c| c.has_chunk_id(id))
    }
    fn has_chunk_id_raw(&self, id: FIoChunkIdRaw) -> bool {
        self.containers.iter().any(|c| c.has_chunk_id_raw(id))
    }
    fn chunks(&self) -> Box<dyn Iterator<Item = ChunkInfo<'_>> + Send + '_> {
        let mut seen = HashSet::new();
        Box::new(self.chunks_all().filter(move |c| seen.insert(c.id())))
    }
    fn chunks_all(&self) -> Box<dyn Iterator<Item = ChunkInfo<'_>> + Send + '_> {
        Box::new(self.containers.iter().flat_map(|c| c.chunks_all()))
    }
    fn packages(&self) -> Box<dyn Iterator<Item = PackageInfo<'_>> + Send + '_> {
        Box::new(self.containers.iter().flat_map(|c| c.packages()))
    }
    fn packages_all(&self) -> Box<dyn Iterator<Item = PackageInfo<'_>> + Send + '_> {
        let mut seen = HashSet::new();
        Box::new(self.packages().filter(move |p| seen.insert(p.id())))
    }
    fn child_containers(&self) -> Box<dyn Iterator<Item = &dyn IoStoreTrait> + '_> {
        Box::new(self.containers.iter().map(Box::as_ref))
    }
    fn chunk_path(&self, id: FIoChunkId) -> Option<String> {
        self.containers.iter().find_map(|c| c.chunk_path(id))
    }
    fn package_store_entry(&self, id: FPackageId) -> Option<StoreEntry> {
        self.containers
            .iter()
            .find_map(|c| c.package_store_entry(id))
    }
    fn lookup_package_redirect(&self, id: FPackageId) -> Option<FPackageId> {
        self.containers
            .iter()
            .find_map(|c| c.lookup_package_redirect(id))
    }
}
