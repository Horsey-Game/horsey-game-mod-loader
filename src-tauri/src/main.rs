#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

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
}

fn find_game() -> Option<PathBuf> {
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
        return Status { game_dir: None, mods: Vec::new() };
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
    }
}

#[tauri::command]
fn toggle_mod(dir: String) -> Result<(), String> {
    let game = find_game().ok_or("Horsey Game was not found")?;
    let mods_dir = game.join("mods");
    let target = PathBuf::from(&dir);
    if !target.starts_with(&mods_dir) {
        return Err("That folder is not inside the Horsey mods folder".into());
    }
    let manifest = target.join("manifest.json");
    let disabled = target.join("manifest.json.disabled");
    let result = if manifest.exists() {
        fs::rename(&manifest, &disabled)
    } else {
        fs::rename(&disabled, &manifest)
    };
    result.map_err(|e| e.to_string())
}

const SHIM_API_VERSION: i64 = 1;
const SHIM_DLL: &[u8] = include_bytes!("../resources/version.dll");
const BACKUPS_KEEP: usize = 5;

#[derive(Serialize)]
struct Installed {
    id: String,
    name: String,
    shim_installed: bool,
}

fn shim_state(game: &Path) -> &'static str {
    match fs::read(game.join("version.dll")) {
        Err(_) => "Not installed",
        Ok(bytes) if bytes == SHIM_DLL => "Installed",
        Ok(_) => "Other version.dll",
    }
}

fn ensure_shim(game: &Path) -> Result<bool, String> {
    if shim_state(game) == "Installed" {
        return Ok(false);
    }
    fs::write(game.join("version.dll"), SHIM_DLL).map_err(|e| {
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
    let extra = dirs.len().saturating_sub(BACKUPS_KEEP);
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
            return Err(format!("unsafe path in zip: {}", entry.name()));
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
        let name = archive.by_index(i).map_err(|e| e.to_string())?.name().to_string();
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
    let mods_dir = game.join("mods");
    let target = PathBuf::from(&dir);
    if target == mods_dir || !target.starts_with(&mods_dir) {
        return Err("That folder is not inside the Horsey mods folder".into());
    }
    backup_game(&game)?;
    fs::remove_dir_all(&target).map_err(|e| e.to_string())
}

const MOD_SERVER: &str = "https://mods.badasswizard.boats";
const INDEX_URL: &str = "https://mods.badasswizard.boats/index.json";

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
    mods: Vec<RemoteMod>,
}

const INDEX_PUBLIC_KEY_HEX: &str =
    "58a9b3f690eff8f58a116e4bf309491790e8e8ee60034854a0beaed067c36f97";
const INDEX_SIG_URL: &str = "https://mods.badasswizard.boats/index.sig";

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
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let index_bytes = http().get(INDEX_URL).send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    let sig_text = http().get(INDEX_SIG_URL).send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;

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
    Ok(index.mods)
}

#[tauri::command]
async fn install_remote(url: String, sha256: String, size: u64, overwrite: bool) -> Result<Installed, String> {
    use sha2::{Digest, Sha256};

    if !url.starts_with(MOD_SERVER) {
        return Err("download URL is not from the mod server".into());
    }
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
    let path = std::env::temp_dir().join(format!("horsey-download-{actual}.zip"));
    fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    let result = install_mod(path.display().to_string(), overwrite);
    let _ = fs::remove_file(&path);
    result
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            load_status,
            toggle_mod,
            install_mod,
            uninstall_mod,
            restore_vanilla,
            install_shim,
            fetch_index,
            install_remote
        ])
        .run(tauri::generate_context!())
        .expect("error while running Horsey Mod Manager");
}


fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("HorseyModManager/1.0")
        .build()
        .expect("http client")
}


fn sequence_file() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|dir| std::path::PathBuf::from(dir).join("HorseyModManager").join("index_sequence.txt"))
}
