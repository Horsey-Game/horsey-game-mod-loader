#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod tools;

use serde::Serialize;
use std::os::windows::process::CommandExt;
use std::{
    fs,
    path::{Path, PathBuf},
};

const STEAM_LIBRARIES: &str = r"C:\Program Files (x86)\Steam\steamapps\libraryfolders.vdf";
const GAME_FOLDER: &str = r"steamapps\common\Horsey Game";

#[derive(Serialize)]
struct ModRow {
    folder: String,
    name: String,
    version: String,
    state: String,
    dir: String,
}

#[derive(Serialize)]
struct Status {
    game_dir: Option<String>,
    mods: Vec<ModRow>,
    horsey_running: bool,
}

// einstellungen aus settings.json, fehlende felder bekommen die standardwerte
#[derive(Serialize, serde::Deserialize)]
#[serde(default)]
struct Settings {
    // leer heißt: über steam suchen
    game_dir: String,
    check_updates: bool,
    backups_keep: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            game_dir: String::new(),
            check_updates: true,
            backups_keep: 5,
        }
    }
}

fn app_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("HorseyModManager"))
}

fn load_settings() -> Settings {
    app_dir()
        .and_then(|dir| fs::read_to_string(dir.join("settings.json")).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn find_game() -> Option<PathBuf> {
    // eigener ordner aus den einstellungen hat vorrang vor steam
    let custom = load_settings().game_dir;
    if !custom.trim().is_empty() {
        let dir = PathBuf::from(custom.trim());
        return dir.join("Horsey.exe").exists().then_some(dir);
    }
    let text = fs::read_to_string(STEAM_LIBRARIES).ok()?;
    for line in text.lines() {
        let parts: Vec<&str> = line.split('"').collect();
        if parts.len() >= 4 && parts[1] == "path" {
            let library = parts[3].replace("\\\\", "\\");
            let dir = PathBuf::from(library).join(GAME_FOLDER);
            if dir.join("Horsey.exe").exists() {
                return Some(dir);
            }
        }
    }
    None
}

fn read_mod(dir: &Path) -> ModRow {
    let folder = dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let manifest = dir.join("manifest.json");
    let disabled = dir.join("manifest.json.disabled");
    let file = if manifest.exists() {
        manifest
    } else if disabled.exists() {
        disabled
    } else {
        return ModRow {
            name: folder.clone(),
            folder,
            version: String::new(),
            state: "No manifest".into(),
            dir: dir.display().to_string(),
        };
    };
    let state = if file.ends_with("manifest.json") { "Enabled" } else { "Disabled" };
    let json: Option<serde_json::Value> = fs::read_to_string(&file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let (name, version, state) = match json {
        Some(v) => (
            v["name"].as_str().unwrap_or(&folder).to_string(),
            v["version"].as_str().unwrap_or("").to_string(),
            state.to_string(),
        ),
        None => (folder.clone(), String::new(), "Bad manifest".to_string()),
    };
    ModRow {
        folder,
        name,
        version,
        state,
        dir: dir.display().to_string(),
    }
}

#[tauri::command]
fn load_status() -> Status {
    let Some(game) = find_game() else {
        return Status { game_dir: None, mods: Vec::new(), horsey_running: horsey_running() };
    };
    let mut mods: Vec<ModRow> = fs::read_dir(game.join("mods"))
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .map(|p| read_mod(&p))
                .collect()
        })
        .unwrap_or_default();
    mods.sort_by_key(|m| m.name.to_lowercase());
    mods.insert(
        0,
        ModRow {
            folder: "version.dll".into(),
            name: "Horsey Shim".into(),
            version: "1.0.0".into(),
            state: shim_state(&game).into(),
            dir: String::new(),
        },
    );
    Status {
        game_dir: Some(game.display().to_string()),
        mods,
        horsey_running: horsey_running(),
    }
}

fn mod_folder(game: &Path, dir: &str) -> Option<PathBuf> {
    let mods = fs::canonicalize(game.join("mods")).ok()?;
    let target = fs::canonicalize(dir).ok()?;
    (target != mods && target.starts_with(&mods)).then_some(target)
}

#[tauri::command]
fn toggle_mod(dir: String) -> Result<(), String> {
    let game = find_game().ok_or("Horsey Game was not found")?;
    let target = mod_folder(&game, &dir).ok_or("That folder is not inside the Horsey mods folder")?;
    let manifest = target.join("manifest.json");
    let disabled = target.join("manifest.json.disabled");
    let result = if manifest.exists() {
        fs::rename(&manifest, &disabled)
    } else {
        fs::rename(&disabled, &manifest)
    };
    result.map_err(|e| e.to_string())
}

const SHIM_API_VERSION: i64 = 2;
const SHIM_DLL: &[u8] = include_bytes!("../resources/version.dll");
#[derive(Serialize)]
struct Installed {
    id: String,
    name: String,
    shim_installed: bool,
}

fn shim_state(game: &Path) -> &'static str {
    match fs::read(game.join("version.dll")) {
        Err(_) => "Not installed",
        Ok(bytes) if is_our_shim(&bytes) => "Installed",
        // horsey-shim, aber nicht die gerade bekannte version
        Ok(bytes) if is_horsey_shim(&bytes) => "Shim update",
        Ok(_) => "Other version.dll",
    }
}

// marker aus dem konsolen-banner, so erkennen wir jede horsey-shim-version
fn is_horsey_shim(bytes: &[u8]) -> bool {
    bytes.windows(11).any(|w| w == b"Horsey Shim")
}

fn shim_cache() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|dir| PathBuf::from(dir).join("HorseyModManager").join("shim").join("version.dll"))
}

fn is_our_shim(bytes: &[u8]) -> bool {
    bytes == SHIM_DLL || shim_cache().and_then(|p| fs::read(p).ok()).is_some_and(|c| c == bytes)
}

fn shim_bytes() -> Vec<u8> {
    shim_cache().and_then(|p| fs::read(p).ok()).unwrap_or_else(|| SHIM_DLL.to_vec())
}

fn ensure_shim(game: &Path) -> Result<bool, String> {
    // eine horsey-shim nie still überschreiben
    if matches!(shim_state(game), "Installed" | "Shim update") {
        return Ok(false);
    }
    fs::write(game.join("version.dll"), shim_bytes()).map_err(|e| {
        format!("could not write version.dll (is Horsey still running?): {e}")
    })?;
    Ok(true)
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn backup_root() -> Result<PathBuf, String> {
    Ok(PathBuf::from(std::env::var("LOCALAPPDATA").map_err(|e| e.to_string())?)
        .join("HorseyModManager")
        .join("backups"))
}

fn backup_game(game: &Path) -> Result<(), String> {
    let root = backup_root()?;
    let vanilla = root.join("vanilla");
    if !vanilla.exists() {
        copy_dir(game, &vanilla).map_err(|e| format!("vanilla backup failed: {e}"))?;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    copy_dir(game, &root.join(format!("backup-{stamp}")))
        .map_err(|e| format!("backup failed: {e}"))?;
    prune_backups(&root);
    Ok(())
}

fn prune_backups(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("backup-"))
        })
        .collect();
    dirs.sort();
    // anzahl kommt aus den einstellungen, 1 bis 20
    let keep = load_settings().backups_keep.clamp(1, 20);
    let extra = dirs.len().saturating_sub(keep);
    for old in dirs.into_iter().take(extra) {
        let _ = fs::remove_dir_all(old);
    }
}

fn extract(
    archive: &mut zip::ZipArchive<fs::File>,
    prefix: &str,
    dest: &Path,
) -> Result<(), String> {
    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(enclosed) = entry.enclosed_name() else {
            return Err(format!(
                "unsafe path in zip: {}",
                entry.name().map(|n| n.to_string()).unwrap_or_default()
            ));
        };
        let Ok(rel) = enclosed.strip_prefix(prefix) else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut file = fs::File::create(&out).map_err(|e| e.to_string())?;
            let remaining = MAX_EXTRACT_BYTES - total;
            let n = std::io::copy(&mut std::io::Read::take(&mut entry, remaining + 1), &mut file)
                .map_err(|e| e.to_string())?;
            total += n;
            if total > MAX_EXTRACT_BYTES {
                return Err("mod is too large when unpacked, not installing".into());
            }
        }
    }
    Ok(())
}

#[tauri::command]
fn install_mod(zip_path: String, overwrite: bool) -> Result<Installed, String> {
    let game = find_game().ok_or("Horsey Game was not found")?;
    let mods_dir = game.join("mods");
    let file = fs::File::open(&zip_path).map_err(|e| e.to_string())?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Not a valid zip: {e}"))?;

    let mut prefix: Option<String> = None;
    for i in 0..archive.len() {
        let name = archive
            .by_index(i)
            .map_err(|e| e.to_string())?
            .name()
            .map_err(|e| e.to_string())?
            .to_string();
        if name == "manifest.json" {
            prefix = Some(String::new());
            break;
        }
        if let Some(p) = name.strip_suffix("/manifest.json") {
            if !p.contains('/') {
                prefix = Some(format!("{p}/"));
                break;
            }
        }
    }
    let prefix = prefix.ok_or("No manifest.json found in this zip")?;

    let manifest_text = {
        let mut entry = archive
            .by_name(&format!("{prefix}manifest.json"))
            .map_err(|e| e.to_string())?;
        let mut text = String::new();
        std::io::Read::read_to_string(&mut entry, &mut text).map_err(|e| e.to_string())?;
        text
    };
    let manifest: serde_json::Value = serde_json::from_str(&manifest_text)
        .map_err(|e| format!("manifest.json is not valid JSON: {e}"))?;

    let id = manifest["id"].as_str().unwrap_or("").to_string();
    let name = manifest["name"].as_str().unwrap_or(&id).to_string();
    let api = manifest["api_version"].as_i64().unwrap_or(0);
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("manifest id may only contain letters, numbers, - and _".into());
    }
    if api < 1 || api > SHIM_API_VERSION {
        return Err(format!(
            "mod needs api_version {api}, this manager supports 1 to {SHIM_API_VERSION}"
        ));
    }

    let dest = mods_dir.join(&id);
    if dest.exists() && !overwrite {
        return Err("already installed".into());
    }

    backup_game(&game)?;
    let shim_installed = ensure_shim(&game)?;

    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(&dest).map_err(|e| e.to_string())?;

    if let Err(e) = extract(&mut archive, &prefix, &dest) {
        let _ = fs::remove_dir_all(&dest);
        return Err(e);
    }
    // stand der dateien merken, für die integritätsprüfung
    tools::record_integrity(&dest, &id);
    Ok(Installed {
        id,
        name,
        shim_installed,
    })
}

fn horsey_running() -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq Horsey.exe", "/NH"])
        .creation_flags(0x0800_0000)
        .output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .to_lowercase()
                .contains("horsey.exe")
        })
        .unwrap_or(false)
}

#[tauri::command]
fn restore_vanilla() -> Result<(), String> {
    if horsey_running() {
        return Err("Close Horsey first".into());
    }
    let game = find_game().ok_or("Horsey Game was not found")?;
    let vanilla = backup_root()?.join("vanilla");
    if !vanilla.exists() {
        return Err("No vanilla backup yet. It is made the first time you install a mod.".into());
    }
    backup_game(&game)?;

    let game_mods = game.join("mods");
    if game_mods.exists() {
        fs::remove_dir_all(&game_mods).map_err(|e| e.to_string())?;
    }
    let vanilla_mods = vanilla.join("mods");
    if vanilla_mods.exists() {
        copy_dir(&vanilla_mods, &game_mods).map_err(|e| e.to_string())?;
    } else {
        fs::create_dir_all(&game_mods).map_err(|e| e.to_string())?;
    }
    let vanilla_dll = vanilla.join("version.dll");
    if vanilla_dll.exists() {
        fs::copy(&vanilla_dll, game.join("version.dll")).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn update_shim(force: bool) -> Result<(), String> {
    let game = find_game().ok_or("Horsey Game was not found")?;
    let index = match load_index().await {
        Ok(index) => index,
        Err(err) => {
            if shim_state(&game) == "Not installed" {
                if horsey_running() {
                    return Err("Horsey is running. Close it and reopen the manager to install the shim".into());
                }
                ensure_shim(&game)?;
            }
            return Err(err);
        }
    };
    let Some(entry) = index.shim else {
        return Ok(());
    };
    let state = shim_state(&game);
    // automatisch nur bei fehlender shim, updates nur mit force aus dem popup
    if state == "Other version.dll" || (!force && state != "Not installed") {
        return Ok(());
    }
    if horsey_running() {
        return Err("Horsey is running. Close it and reopen the manager to install the shim".into());
    }
    // vor dem überschreiben sichern
    if force {
        backup_game(&game)?;
    }
    // größe und hash werden beim streamen geprüft
    let bytes = download_checked(&entry.download_url, entry.size, &entry.sha256).await?;
    let cache = shim_cache().ok_or("could not find the local app data folder")?;
    if let Some(dir) = cache.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&cache, &bytes).map_err(|e| e.to_string())?;
    fs::write(game.join("version.dll"), &bytes)
        .map_err(|e| format!("could not write version.dll (is Horsey still running?): {e}"))?;
    Ok(())
}

#[tauri::command]
fn install_shim() -> Result<(), String> {
    if horsey_running() {
        return Err("Close Horsey first".into());
    }
    let game = find_game().ok_or("Horsey Game was not found")?;
    if shim_state(&game) == "Installed" {
        return Ok(());
    }
    backup_game(&game)?;
    ensure_shim(&game)?;
    Ok(())
}

#[tauri::command]
fn uninstall_mod(dir: String) -> Result<(), String> {
    let game = find_game().ok_or("Horsey Game was not found")?;
    let target = mod_folder(&game, &dir).ok_or("That folder is not inside the Horsey mods folder")?;
    backup_game(&game)?;
    fs::remove_dir_all(&target).map_err(|e| e.to_string())
}

const INDEX_URL: &str = "https://mods.badasswizard.boats/index.json";

fn from_mod_server(url: &str) -> bool {
    reqwest::Url::parse(url)
        .map(|u| u.scheme() == "https" && u.host_str() == Some("mods.badasswizard.boats"))
        .unwrap_or(false)
}

#[derive(Serialize, serde::Deserialize, Clone)]
struct RemoteMod {
    id: String,
    name: String,
    version: String,
    author: String,
    description: String,
    api_version: i64,
    dependencies: Vec<serde_json::Value>,
    sha256: String,
    size: u64,
    download_url: String,
}

#[derive(serde::Deserialize)]
struct IndexFile {
    #[serde(default)]
    sequence: u64,
    #[serde(default)]
    shim: Option<ShimEntry>,
    // neue manager-version, fehlt bei alten listen
    #[serde(default)]
    manager: Option<ManagerEntry>,
    mods: Vec<RemoteMod>,
}

#[derive(serde::Deserialize)]
struct ShimEntry {
    sha256: String,
    size: u64,
    download_url: String,
}

// eintrag für ein manager-update aus der signierten liste
#[derive(serde::Deserialize)]
struct ManagerEntry {
    version: String,
    sha256: String,
    size: u64,
    download_url: String,
}

// was das popup braucht: neue und aktuelle version
#[derive(serde::Serialize)]
struct ManagerUpdate {
    version: String,
    current: String,
}

// vergleicht versionen zahlenweise, z.b. 0.1.10 ist neuer als 0.1.9
fn version_newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.split('.').map(|p| p.trim().parse().unwrap_or(0)).collect()
    };
    parse(a) > parse(b)
}

const INDEX_PUBLIC_KEY_HEX: &str =
    "58a9b3f690eff8f58a116e4bf309491790e8e8ee60034854a0beaed067c36f97";
const INDEX_SIG_URL: &str = "https://mods.badasswizard.boats/index.sig";

// bei jedem manager-release hochsetzen, dann lassen sich alte listen nicht mehr einspielen
const MIN_INDEX_SEQUENCE: u64 = 21;

const MAX_DOWNLOAD_BYTES: u64 = 50 * 1024 * 1024;
const MAX_EXTRACT_BYTES: u64 = 200 * 1024 * 1024;

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[tauri::command]
async fn fetch_index() -> Result<Vec<RemoteMod>, String> {
    Ok(load_index().await?.mods)
}

async fn load_index() -> Result<IndexFile, String> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    // netz zuerst, bei fehler die letzte gespeicherte liste (die wird unten trotzdem geprüft)
    let fetched = async {
        let index_bytes = http().get(INDEX_URL).send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .bytes()
            .await
            .map_err(|e| e.to_string())?
            .to_vec();
        let sig_text = http().get(INDEX_SIG_URL).send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        Ok::<_, String>((index_bytes, sig_text))
    }
    .await;
    let (index_bytes, sig_text) = match fetched {
        Ok(files) => files,
        Err(err) => {
            let Some(files) = read_index_cache() else {
                return Err(err);
            };
            log_file("app.log", &format!("mod list offline, using saved copy: {err}"));
            files
        }
    };

    let key_bytes: [u8; 32] = hex_decode(INDEX_PUBLIC_KEY_HEX)
        .and_then(|v| v.try_into().ok())
        .ok_or("the app has a bad public key")?;
    let key = VerifyingKey::from_bytes(&key_bytes).map_err(|e| e.to_string())?;
    let sig_bytes = hex_decode(sig_text.trim()).ok_or("the mod list signature is malformed")?;
    let signature = Signature::from_slice(&sig_bytes).map_err(|e| e.to_string())?;
    key.verify(&index_bytes, &signature)
        .map_err(|_| "the mod list signature does not match, not loading it".to_string())?;

    let index: IndexFile = serde_json::from_slice(&index_bytes)
        .map_err(|e| format!("mod list is not valid: {e}"))?;
    // alte signierte listen werden auch bei neuer installation abgelehnt
    if index.sequence < MIN_INDEX_SEQUENCE {
        return Err("mod list is older than this manager allows, not loading it".into());
    }
    if let Some(file) = sequence_file() {
        let last: u64 = fs::read_to_string(&file)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        if index.sequence < last {
            return Err("mod list is older than one already accepted, not loading it".into());
        }
        if index.sequence > last {
            if let Some(dir) = file.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(&file, index.sequence.to_string());
        }
    }
    // geprüfte liste merken, für den offline-fall
    save_index_cache(&index_bytes, &sig_text);
    Ok(index)
}

fn index_cache_dir() -> Option<PathBuf> {
    app_dir().map(|dir| dir.join("cache"))
}

fn read_index_cache() -> Option<(Vec<u8>, String)> {
    let dir = index_cache_dir()?;
    let bytes = fs::read(dir.join("index.json")).ok()?;
    let sig = fs::read_to_string(dir.join("index.sig")).ok()?;
    Some((bytes, sig))
}

fn save_index_cache(bytes: &[u8], sig: &str) {
    let Some(dir) = index_cache_dir() else {
        return;
    };
    let _ = fs::create_dir_all(&dir);
    let _ = fs::write(dir.join("index.json"), bytes);
    let _ = fs::write(dir.join("index.sig"), sig);
}

#[tauri::command]
async fn install_remote(url: String, overwrite: bool) -> Result<Installed, String> {
    use sha2::{Digest, Sha256};

    if !from_mod_server(&url) {
        return Err("download URL is not from the mod server".into());
    }
    let index = load_index().await?;
    let listed = index
        .mods
        .iter()
        .find(|m| m.download_url == url)
        .ok_or("that mod is not in the signed mod list")?;
    let sha256 = listed.sha256.clone();
    let size = listed.size;
    if size > MAX_DOWNLOAD_BYTES {
        return Err("mod is larger than the mod manager allows, not downloading".into());
    }
    let mut resp = http().get(&url).send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let mut bytes: Vec<u8> = Vec::with_capacity(size as usize);
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len() as u64 + chunk.len() as u64 > size {
            return Err("download is larger than the index says, not installing".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() as u64 != size {
        return Err("download size does not match the index, not installing".into());
    }
    let actual: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != sha256.to_lowercase() {
        return Err("checksum does not match the index, not installing".into());
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let path = std::env::temp_dir().join(format!("horsey-download-{}-{nonce}.zip", std::process::id()));
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut file, &bytes).map_err(|e| e.to_string())?;
    }
    let result = install_mod(path.display().to_string(), overwrite);
    let _ = fs::remove_file(&path);
    result
}

#[tauri::command]
async fn shim_update_available() -> Result<bool, String> {
    use sha2::{Digest, Sha256};

    let game = find_game().ok_or("Horsey Game was not found")?;
    let index = load_index().await?;
    let Some(entry) = index.shim else {
        return Ok(false);
    };
    let state = shim_state(&game);
    if state != "Installed" && state != "Shim update" {
        return Ok(false);
    }
    let installed = fs::read(game.join("version.dll")).map_err(|e| e.to_string())?;
    let hash: String = Sha256::digest(&installed).iter().map(|b| format!("{b:02x}")).collect();
    // anderer hash heißt: update verfügbar
    if hash != entry.sha256.to_lowercase() {
        return Ok(true);
    }
    // gleiche version wie die liste, also als installiert merken
    if state == "Shim update" {
        if let Some(cache) = shim_cache() {
            if let Some(dir) = cache.parent() {
                fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            fs::write(&cache, &installed).map_err(|e| e.to_string())?;
        }
    }
    Ok(false)
}

// nur anbieten wenn die signierte liste eine neuere version hat
#[tauri::command]
async fn manager_update_available() -> Result<Option<ManagerUpdate>, String> {
    let index = load_index().await?;
    let Some(entry) = index.manager else {
        return Ok(None);
    };
    let current = env!("CARGO_PKG_VERSION").to_string();
    if version_newer(&entry.version, &current) {
        Ok(Some(ManagerUpdate { version: entry.version, current }))
    } else {
        Ok(None)
    }
}

// installer laden, prüfen, starten und danach den manager beenden
#[tauri::command]
async fn apply_manager_update() -> Result<(), String> {
    let index = load_index().await?;
    let entry = index.manager.ok_or("no manager update in the mod list")?;
    // das backend prüft die version selbst, nicht nur das popup
    if !version_newer(&entry.version, env!("CARGO_PKG_VERSION")) {
        return Err("the mod list has no newer manager version".into());
    }
    update_log(&format!("update to {} requested", entry.version));
    let bytes = match download_checked(&entry.download_url, entry.size, &entry.sha256).await {
        Ok(b) => {
            update_log(&format!("download ok, {} bytes", b.len()));
            b
        }
        Err(e) => {
            update_log(&format!("download failed: {e}"));
            return Err(e);
        }
    };
    // jeder lauf bekommt einen neuen ordner, der nicht vorher existieren darf
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("horsey-mm-update-{}-{nonce}", std::process::id()));
    fs::create_dir(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("horsey-mod-manager-{}-setup.exe", entry.version));
    fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    update_log(&format!("installer written to {}", path.display()));
    let verb = wide("runas");
    let file = wide(&path.to_string_lossy());
    let code = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    };
    if code <= 32 {
        update_log(&format!("installer not started, ShellExecute returned {code}"));
        return Err(format!(
            "the installer did not start (code {code}). If the admin prompt was declined, run the update again and click Yes."
        ));
    }
    update_log(&format!("installer started elevated, ShellExecute returned {code}"));
    update_log("manager exiting for the install");
    // manager beenden, damit der installer die exe ersetzen kann
    std::process::exit(0)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *mut std::ffi::c_void,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
    ) -> isize;
}

fn update_log(line: &str) {
    log_file("update.log", line)
}

fn log_file(name: &str, line: &str) {
    use std::io::Write;
    let Some(base) = std::env::var_os("LOCALAPPDATA") else { return };
    let dir = std::path::PathBuf::from(base).join("com.chris.horseymodmanager");
    let _ = fs::create_dir_all(&dir);
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join(name)) {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "{secs} {line}");
    }
}

#[tauri::command]
fn app_log(line: String) {
    log_file("app.log", &line);
}

#[tauri::command]
fn get_settings() -> Settings {
    load_settings()
}

#[tauri::command]
fn save_settings(settings: Settings) -> Result<(), String> {
    // ordner muss ein horsey sein, sonst nichts speichern
    let game_dir = settings.game_dir.trim().to_string();
    if !game_dir.is_empty() && !PathBuf::from(&game_dir).join("Horsey.exe").exists() {
        return Err("There is no Horsey.exe in that folder".into());
    }
    let saved = Settings {
        game_dir,
        check_updates: settings.check_updates,
        backups_keep: settings.backups_keep.clamp(1, 20),
    };
    let dir = app_dir().ok_or("could not find the local app data folder")?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&saved).map_err(|e| e.to_string())?;
    fs::write(dir.join("settings.json"), text).map_err(|e| e.to_string())
}

fn log_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(|dir| PathBuf::from(dir).join("com.chris.horseymodmanager"))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[tauri::command]
fn open_log_dir() -> Result<(), String> {
    let dir = log_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer")
        .arg(&dir)
        .creation_flags(0x0800_0000)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// letzte zeilen einer logdatei, lesefehler kommen als text zurück
fn tail(path: &Path, lines: usize) -> String {
    match fs::read(path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let all: Vec<&str> = text.lines().collect();
            let start = all.len().saturating_sub(lines);
            let mut out = all[start..].join("\n");
            out.push('\n');
            out
        }
        Err(e) => format!("(could not read {}: {e})\n", path.display()),
    }
}

// schreibt einen bericht für fehlersuche, gibt den pfad zurück
#[tauri::command]
async fn export_diagnostics() -> Result<String, String> {
    use sha2::{Digest, Sha256};

    let settings = load_settings();
    let status = load_status();
    let mut out = format!(
        "Horsey Mod Manager {}\nhorsey running: {}\ngame: {}\nsettings: game_dir={:?} check_updates={} backups_keep={}\n",
        env!("CARGO_PKG_VERSION"),
        status.horsey_running,
        status.game_dir.as_deref().unwrap_or("not found"),
        settings.game_dir,
        settings.check_updates,
        settings.backups_keep,
    );
    out.push_str("\nmods:\n");
    for m in &status.mods {
        out.push_str(&format!("  {} | {} | {} | {}\n", m.folder, m.name, m.version, m.state));
    }
    if let Some(game) = find_game() {
        if let Ok(exe) = fs::read(game.join("Horsey.exe")) {
            let hash: String = Sha256::digest(&exe).iter().map(|b| format!("{b:02x}")).collect();
            out.push_str(&format!("\nHorsey.exe: {} bytes, sha256 {hash}\n", exe.len()));
        }
        out.push_str(&format!(
            "\n--- SHIM.log (last 200 lines) ---\n{}",
            tail(&game.join("SHIM.log"), 200)
        ));
    }
    for name in ["app.log", "update.log"] {
        out.push_str(&format!(
            "\n--- {name} (last 200 lines) ---\n{}",
            tail(&log_dir().join(name), 200)
        ));
    }
    let dir = log_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("diagnostics.txt");
    fs::write(&path, out).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}


// --- infos pro mod für die oberfläche: api, abhängigkeiten, config ---
#[derive(Serialize)]
struct ModExtra {
    folder: String,
    id: String,
    api_version: i64,
    deps: Vec<String>,
    has_config: bool,
    // optional: beschreibung der config-felder aus dem manifest (label, min, max)
    config_schema: serde_json::Value,
}

#[derive(Serialize)]
struct Extras {
    shim_api: i64,
    mods: Vec<ModExtra>,
}

// manifest lesen, egal ob der mod an oder aus ist
fn read_manifest(dir: &Path) -> Option<serde_json::Value> {
    let file = if dir.join("manifest.json").exists() {
        dir.join("manifest.json")
    } else {
        dir.join("manifest.json.disabled")
    };
    serde_json::from_str(&fs::read_to_string(file).ok()?).ok()
}

// abhängigkeit als id, akzeptiert "id" oder {"id": ...}
fn dep_id(v: &serde_json::Value) -> Option<String> {
    v.as_str()
        .map(str::to_string)
        .or_else(|| v["id"].as_str().map(str::to_string))
}

fn mod_dirs(game: &Path) -> Vec<PathBuf> {
    fs::read_dir(game.join("mods"))
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

#[tauri::command]
fn mod_extras() -> Extras {
    let mods = find_game()
        .map(|game| {
            mod_dirs(&game)
                .iter()
                .map(|dir| {
                    let folder = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    let manifest = read_manifest(dir).unwrap_or(serde_json::Value::Null);
                    ModExtra {
                        id: manifest["id"].as_str().unwrap_or(&folder).to_string(),
                        api_version: manifest["api_version"].as_i64().unwrap_or(0),
                        deps: manifest["dependencies"]
                            .as_array()
                            .map(|a| a.iter().filter_map(dep_id).collect())
                            .unwrap_or_default(),
                        has_config: dir.join("config.lua").exists(),
                        config_schema: manifest["config_schema"].clone(),
                        folder,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Extras { shim_api: SHIM_API_VERSION, mods }
}

// --- config editor: config.lua, nur einfache werte (bool, zahl, text) ---
#[derive(Serialize, serde::Deserialize)]
struct ConfigField {
    key: String,
    kind: String,
    value: String,
}

fn is_lua_number(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_digit() || "+-.eE".contains(c))
        && s.parse::<f64>().is_ok()
}

// eine zeile wie `    speed = 2,` lesen, alles andere bleibt unangetastet
fn parse_config_line(line: &str) -> Option<ConfigField> {
    let (key, rest) = line.trim().split_once('=')?;
    let key = key.trim();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let value = rest.trim().trim_end_matches(',').trim();
    let (kind, value) = if value == "true" || value == "false" {
        ("bool", value.to_string())
    } else if is_lua_number(value) {
        ("number", value.to_string())
    } else if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        let inner = &value[1..value.len() - 1];
        ("string", inner.replace("\\\"", "\"").replace("\\\\", "\\"))
    } else {
        return None;
    };
    Some(ConfigField {
        key: key.to_string(),
        kind: kind.to_string(),
        value,
    })
}

// feldwert in lua-schreibweise, mit prüfung
fn lua_value(field: &ConfigField) -> Result<String, String> {
    match field.kind.as_str() {
        "bool" if field.value == "true" || field.value == "false" => Ok(field.value.clone()),
        "bool" => Err(format!("{} must be true or false", field.key)),
        "number" if is_lua_number(field.value.trim()) => Ok(field.value.trim().to_string()),
        "number" => Err(format!("{} must be a number", field.key)),
        "string" if field.value.contains(|c| c == '\n' || c == '\r') => {
            Err(format!("{} cannot have line breaks", field.key))
        }
        "string" => Ok(format!(
            "\"{}\"",
            field.value.replace('\\', "\\\\").replace('"', "\\\"")
        )),
        _ => Err(format!("{} has an unknown type", field.key)),
    }
}

fn config_path(dir: &str) -> Result<PathBuf, String> {
    let game = find_game().ok_or("Horsey Game was not found")?;
    let target = mod_folder(&game, dir).ok_or("That folder is not inside the Horsey mods folder")?;
    Ok(target.join("config.lua"))
}

#[tauri::command]
fn read_config(dir: String) -> Result<Vec<ConfigField>, String> {
    let text = fs::read_to_string(config_path(&dir)?).map_err(|e| e.to_string())?;
    Ok(text.lines().filter_map(parse_config_line).collect())
}

// ändert nur die geänderten zeilen, der rest der datei bleibt gleich
#[tauri::command]
fn write_config(dir: String, fields: Vec<ConfigField>) -> Result<(), String> {
    let path = config_path(&dir)?;
    let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut values = std::collections::HashMap::new();
    for f in &fields {
        tools::check_range(path.parent().unwrap_or(Path::new(".")), f)?;
        values.insert(f.key.as_str(), lua_value(f)?);
    }
    let mut out = String::new();
    for line in text.lines() {
        let key = line.split_once('=').map(|(k, _)| k.trim()).unwrap_or("");
        match values.get(key) {
            Some(new) if parse_config_line(line).is_some() => {
                let eq = line.find('=').unwrap_or(0);
                let comma = if line.trim_end().ends_with(',') { "," } else { "" };
                out.push_str(&format!("{} {}{}\n", line[..=eq].trim_end(), new, comma));
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    // alte fassung sichern, bevor sie überschrieben wird
    fs::write(path.with_file_name("config.lua.bak"), &text).map_err(|e| e.to_string())?;
    fs::write(&path, out).map_err(|e| e.to_string())
}

// --- crash-erkennung: exit-code und die letzte mod-zeile im SHIM.log ---
#[derive(Serialize, Clone)]
struct CrashReport {
    code: Option<i32>,
    crashed: bool,
    culprit: Option<String>,
    line: Option<String>,
    // false, wenn horsey nicht aus dem manager gestartet wurde (dann gibt es keinen exit-code)
    exit_known: bool,
}

// (id, name) aller mods, auch der ausgeschalteten
fn mod_names(game: &Path) -> Vec<(String, String)> {
    mod_dirs(game)
        .iter()
        .filter_map(|dir| {
            let folder = dir.file_name()?.to_string_lossy().into_owned();
            let manifest = read_manifest(dir)?;
            let id = manifest["id"].as_str().unwrap_or(&folder).to_string();
            let name = manifest["name"].as_str().unwrap_or(&folder).to_string();
            Some((id, name))
        })
        .collect()
}

// der teil von SHIM.log, der seit dem start dazugekommen ist
fn log_since(game: &Path, offset: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = fs::File::open(game.join("SHIM.log")) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = if len < offset { 0 } else { offset };
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_ok() {
        let _ = file.read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

// die letzte zeile, die von einem mod kommt, ist der verdächtige
fn blame(lines: &[&str], mods: &[(String, String)]) -> Option<(String, String)> {
    for line in lines.iter().rev() {
        for (id, name) in mods {
            let tagged = line.starts_with(&format!("[{id}]"));
            let loaded = line.contains(&format!("loaded mod '{id}'"));
            let path = line.contains(&format!("mods\\{id}\\")) || line.contains(&format!("mods/{id}/"));
            if tagged || loaded || path {
                return Some((name.clone(), line.to_string()));
            }
        }
    }
    None
}

fn crash_report(game: &Path, offset: u64, code: Option<i32>) -> CrashReport {
    let log = log_since(game, offset);
    let lines: Vec<&str> = log.lines().collect();
    let (culprit, line) = match blame(&lines, &mod_names(game)) {
        Some((name, line)) => (Some(name), Some(line)),
        None => (None, None),
    };
    CrashReport {
        code,
        crashed: code != Some(0),
        exit_known: true,
        culprit,
        line,
    }
}

// startet horsey.exe, das ende meldet tools::launch (event horsey-exited)
#[tauri::command]
fn launch_horsey(app: tauri::AppHandle) -> Result<(), String> {
    tools::launch(app, false)
}

// --- crash bisect: mods in hälften aus- und einschalten, bis einer übrig ist ---
#[derive(Serialize, serde::Deserialize)]
struct BisectState {
    original: Vec<String>,
    suspects: Vec<String>,
    testing: Vec<String>,
}

#[derive(Serialize)]
struct BisectStep {
    active: bool,
    testing: Vec<String>,
    suspects_left: usize,
    culprit: Option<String>,
}

fn bisect_file() -> Option<PathBuf> {
    app_dir().map(|dir| dir.join("bisect.json"))
}

fn load_bisect() -> Option<BisectState> {
    let text = fs::read_to_string(bisect_file()?).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_bisect(state: &BisectState) -> Result<(), String> {
    let file = bisect_file().ok_or("could not find the local app data folder")?;
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    fs::write(file, text).map_err(|e| e.to_string())
}

fn clear_bisect() {
    if let Some(file) = bisect_file() {
        let _ = fs::remove_file(file);
    }
}

fn step_view(state: &BisectState, culprit: Option<String>) -> BisectStep {
    BisectStep {
        active: culprit.is_none(),
        testing: state.testing.clone(),
        suspects_left: state.suspects.len(),
        culprit,
    }
}

// an oder aus, je nachdem ob der mod im test ist
fn set_mod_enabled(game: &Path, folder: &str, enabled: bool) -> Result<(), String> {
    let dir = game.join("mods").join(folder);
    let manifest = dir.join("manifest.json");
    let disabled = dir.join("manifest.json.disabled");
    let result = if enabled && disabled.exists() {
        fs::rename(&disabled, &manifest)
    } else if !enabled && manifest.exists() {
        fs::rename(&manifest, &disabled)
    } else {
        Ok(())
    };
    result.map_err(|e| e.to_string())
}

fn apply_test(game: &Path, state: &BisectState) -> Result<(), String> {
    for folder in &state.original {
        set_mod_enabled(game, folder, state.testing.contains(folder))?;
    }
    Ok(())
}

fn restore_all(game: &Path, original: &[String]) -> Result<(), String> {
    for folder in original {
        set_mod_enabled(game, folder, true)?;
    }
    Ok(())
}

// die vordere hälfte der verdächtigen kommt in den test
fn next_test(state: &mut BisectState) {
    let half = (state.suspects.len() + 1) / 2;
    state.testing = state.suspects[..half].to_vec();
}

#[tauri::command]
fn bisect_status() -> Option<BisectStep> {
    load_bisect().map(|state| step_view(&state, None))
}

#[tauri::command]
fn bisect_start() -> Result<BisectStep, String> {
    if horsey_running() {
        return Err("Close Horsey first".into());
    }
    if load_bisect().is_some() {
        return Err("A bisect is already running. Cancel it first.".into());
    }
    let game = find_game().ok_or("Horsey Game was not found")?;
    let enabled: Vec<String> = mod_dirs(&game)
        .iter()
        .filter(|d| d.join("manifest.json").exists())
        .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    if enabled.len() < 2 {
        return Err("Need at least two enabled mods to bisect".into());
    }
    let mut state = BisectState {
        original: enabled.clone(),
        suspects: enabled,
        testing: Vec::new(),
    };
    next_test(&mut state);
    if let Err(e) = apply_test(&game, &state) {
        let _ = restore_all(&game, &state.original);
        return Err(e);
    }
    if let Err(e) = save_bisect(&state) {
        let _ = restore_all(&game, &state.original);
        return Err(e);
    }
    log_file("app.log", &format!("bisect started with {} mods", state.original.len()));
    Ok(step_view(&state, None))
}

#[tauri::command]
fn bisect_answer(crashed: bool) -> Result<BisectStep, String> {
    let game = find_game().ok_or("Horsey Game was not found")?;
    let mut state = load_bisect().ok_or("No bisect is running")?;
    if crashed {
        state.suspects = state.testing.clone();
    } else {
        let testing = state.testing.clone();
        state.suspects.retain(|f| !testing.contains(f));
    }
    if state.suspects.len() == 1 {
        let culprit = state.suspects[0].clone();
        restore_all(&game, &state.original)?;
        clear_bisect();
        log_file("app.log", &format!("bisect found {culprit}"));
        return Ok(step_view(&state, Some(culprit)));
    }
    next_test(&mut state);
    apply_test(&game, &state)?;
    save_bisect(&state)?;
    Ok(step_view(&state, None))
}

#[tauri::command]
fn bisect_cancel() -> Result<(), String> {
    let Some(state) = load_bisect() else {
        return Ok(());
    };
    let game = find_game().ok_or("Horsey Game was not found")?;
    restore_all(&game, &state.original)?;
    clear_bisect();
    Ok(())
}

fn main() {
    std::panic::set_hook(Box::new(|info| log_file("app.log", &format!("panic: {info}"))));
    log_file("app.log", &format!("start {}", env!("CARGO_PKG_VERSION")));
    tauri::Builder::default()
        .setup(|app| {
            // steam-starts erkennen und safe mode nach einem absturz aufräumen
            tools::start_watch(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_status,
            toggle_mod,
            install_mod,
            uninstall_mod,
            restore_vanilla,
            install_shim,
            fetch_index,
            install_remote,
            update_shim,
            shim_update_available,
            manager_update_available,
            apply_manager_update,
            app_log,
            get_settings,
            save_settings,
            open_log_dir,
            export_diagnostics,
            launch_horsey,
            mod_extras,
            read_config,
            write_config,
            bisect_status,
            bisect_start,
            bisect_answer,
            bisect_cancel,
            tools::check_integrity,
            tools::record_baseline,
            tools::list_backups,
            tools::restore_backup,
            tools::list_profiles,
            tools::save_profile,
            tools::apply_profile,
            tools::delete_profile,
            tools::game_version_status,
            tools::accept_game_version,
            tools::shim_signatures,
            tools::safe_mode_launch,
            tools::crash_history,
            tools::clear_crash_history,
            tools::shim_log,
            tools::load_order,
            tools::save_load_order
        ])
        .run(tauri::generate_context!())
        .expect("error while running Horsey Mod Manager");
}


fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("HorseyModManager/1.0")
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .expect("http client")
}

// lädt eine datei vom mod server, bricht beim überschreiten der größe ab und prüft den hash
async fn download_checked(url: &str, size: u64, sha256: &str) -> Result<Vec<u8>, String> {
    use sha2::{Digest, Sha256};

    if !from_mod_server(url) {
        return Err("download URL is not from the mod server".into());
    }
    if size > MAX_DOWNLOAD_BYTES {
        return Err("download is larger than the mod manager allows, not downloading".into());
    }
    let mut resp = http().get(url).send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let mut bytes: Vec<u8> = Vec::with_capacity(size as usize);
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len() as u64 + chunk.len() as u64 > size {
            return Err("download is larger than the mod list says, not installing".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() as u64 != size {
        return Err("download size does not match the mod list, not installing".into());
    }
    let actual: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if actual != sha256.to_lowercase() {
        return Err("download checksum does not match the mod list, not installing".into());
    }
    Ok(bytes)
}


fn sequence_file() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|dir| std::path::PathBuf::from(dir).join("HorseyModManager").join("index_sequence.txt"))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_is_numeric() {
        assert!(version_newer("0.2.3", "0.2.2"));
        assert!(version_newer("0.10.0", "0.9.9"));
        assert!(!version_newer("0.2.2", "0.2.2"));
        assert!(!version_newer("0.2.1", "0.2.2"));
    }

    #[test]
    fn hex_decodes_pairs_only() {
        assert_eq!(hex_decode("00ff10"), Some(vec![0x00, 0xff, 0x10]));
        assert_eq!(hex_decode("abc"), None);
        assert_eq!(hex_decode("zz"), None);
    }

    #[test]
    fn download_host_must_be_https_mod_server() {
        assert!(from_mod_server("https://mods.badasswizard.boats/files/autonamer-0.1.2.zip"));
        assert!(!from_mod_server("http://mods.badasswizard.boats/files/autonamer-0.1.2.zip"));
        assert!(!from_mod_server("https://mods.badasswizard.boats.example.com/files/a.zip"));
        assert!(!from_mod_server("https://example.com/files/a.zip"));
        assert!(!from_mod_server("not a url"));
    }
}