// werkzeuge: integrität, backups, profile, spielversion, shim-signaturen, safe mode, steam-erkennung
use super::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

// läuft gerade ein start aus dem manager, dann meldet die steam-erkennung nichts
static LAUNCH_ACTIVE: AtomicBool = AtomicBool::new(false);

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    fs::write(path, text).map_err(|e| e.to_string())
}

fn sha_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn file_sha(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|b| sha_hex(&b))
}

fn data_path(name: &str) -> Option<PathBuf> {
    app_dir().map(|dir| dir.join(name))
}

fn folder_name(dir: &Path) -> String {
    dir.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

// mod-ordner, die gerade an sind
fn enabled_folders(game: &Path) -> Vec<String> {
    mod_dirs(game)
        .iter()
        .filter(|d| d.join("manifest.json").exists())
        .map(|d| folder_name(d))
        .collect()
}

fn safe_path() -> Option<PathBuf> {
    data_path("safe_mode.json")
}

// bisect und safe mode ändern die mods, da darf nichts anderes dazwischenfunken
fn guard_mods() -> Result<(), String> {
    if load_bisect().is_some() {
        return Err("Finish or cancel the bisect first".into());
    }
    if safe_path().is_some_and(|p| p.exists()) {
        return Err("Safe mode is active. Wait for Horsey to close.".into());
    }
    Ok(())
}

// --- integrität: hash jeder datei beim install, später vergleichen ---
#[derive(Serialize, Deserialize, Default)]
struct Baseline {
    files: BTreeMap<String, String>,
}

fn integrity_path(folder: &str) -> Option<PathBuf> {
    app_dir().map(|dir| dir.join("integrity").join(format!("{folder}.json")))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

// dateien, die der nutzer oder das spiel selbst ändert, werden nicht geprüft
fn ignored(rel: &str) -> bool {
    matches!(rel, "manifest.json" | "manifest.json.disabled" | "config.lua" | "config.lua.bak")
}

fn hash_mod(dir: &Path) -> BTreeMap<String, String> {
    let mut files = Vec::new();
    walk(dir, &mut files);
    let mut out = BTreeMap::new();
    for file in files {
        let Ok(rel) = file.strip_prefix(dir) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if ignored(&rel) {
            continue;
        }
        if let Some(hash) = file_sha(&file) {
            out.insert(rel, hash);
        }
    }
    out
}

// nach jeder installation: den stand der dateien merken
pub fn record_integrity(dest: &Path, folder: &str) {
    let Some(path) = integrity_path(folder) else {
        return;
    };
    let _ = write_json(&path, &Baseline { files: hash_mod(dest) });
}

#[derive(Serialize)]
pub struct IntegrityRow {
    folder: String,
    name: String,
    status: String,
    problems: Vec<String>,
}

#[tauri::command]
pub fn check_integrity() -> Vec<IntegrityRow> {
    let Some(game) = find_game() else {
        return Vec::new();
    };
    mod_dirs(&game)
        .iter()
        .map(|dir| {
            let folder = folder_name(dir);
            let name = read_manifest(dir)
                .and_then(|m| m["name"].as_str().map(str::to_string))
                .unwrap_or_else(|| folder.clone());
            let Some(base) = integrity_path(&folder).and_then(|p| read_json::<Baseline>(&p)) else {
                return IntegrityRow {
                    folder,
                    name,
                    status: "no record".into(),
                    problems: Vec::new(),
                };
            };
            let mut problems = Vec::new();
            for (rel, expected) in &base.files {
                let path = dir.join(rel);
                if !path.exists() {
                    problems.push(format!("missing {rel}"));
                } else if file_sha(&path).as_deref() != Some(expected.as_str()) {
                    problems.push(format!("changed {rel}"));
                }
            }
            let status = if problems.is_empty() { "ok" } else { "modified" };
            IntegrityRow {
                folder,
                name,
                status: status.into(),
                problems,
            }
        })
        .collect()
}

// für mods ohne merkstand: den jetzigen stand als richtig festhalten
#[tauri::command]
pub fn record_baseline() -> Result<usize, String> {
    let game = find_game().ok_or("Horsey Game was not found")?;
    let mut count = 0;
    for dir in mod_dirs(&game) {
        let folder = folder_name(&dir);
        let path = integrity_path(&folder).ok_or("could not find the local app data folder")?;
        if !path.exists() {
            record_integrity(&dir, &folder);
            count += 1;
        }
    }
    Ok(count)
}

// --- backups: liste, vergleich mit dem jetzigen stand, zurückspielen ---
#[derive(Serialize)]
pub struct BackupRow {
    name: String,
    stamp: u64,
    vanilla: bool,
    mods: Vec<String>,
    differs: usize,
}

// (ordner, version) der mods in einem mods-ordner
fn mod_versions(mods_dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(entries) = fs::read_dir(mods_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let version = read_manifest(&path)
                .and_then(|m| m["version"].as_str().map(str::to_string))
                .unwrap_or_default();
            out.insert(folder_name(&path), version);
        }
    }
    out
}

#[tauri::command]
pub fn list_backups() -> Result<Vec<BackupRow>, String> {
    let game = find_game().ok_or("Horsey Game was not found")?;
    let current = mod_versions(&game.join("mods"));
    let Ok(entries) = fs::read_dir(backup_root()?) else {
        return Ok(Vec::new());
    };
    let mut rows = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let vanilla = name == "vanilla";
        let stamp = name
            .strip_prefix("backup-")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        if !path.is_dir() || (!vanilla && stamp == 0) {
            continue;
        }
        let versions = mod_versions(&path.join("mods"));
        let changed = versions.iter().filter(|(k, v)| current.get(*k) != Some(*v)).count();
        let removed = current.keys().filter(|k| !versions.contains_key(*k)).count();
        rows.push(BackupRow {
            name,
            stamp,
            vanilla,
            mods: versions.iter().map(|(k, v)| format!("{k} {v}")).collect(),
            differs: changed + removed,
        });
    }
    rows.sort_by(|a, b| b.stamp.cmp(&a.stamp));
    Ok(rows)
}

// spielt mods und version.dll aus einem backup zurück, vorher wird der jetzige stand gesichert
#[tauri::command]
pub fn restore_backup(name: String) -> Result<(), String> {
    if horsey_running() {
        return Err("Close Horsey first".into());
    }
    let valid = name == "vanilla"
        || (name.starts_with("backup-") && name.len() > 7 && name[7..].chars().all(|c| c.is_ascii_digit()));
    if !valid {
        return Err("Not a backup name".into());
    }
    guard_mods()?;
    let game = find_game().ok_or("Horsey Game was not found")?;
    let src = backup_root()?.join(&name);
    if !src.is_dir() {
        return Err("That backup is gone".into());
    }
    backup_game(&game)?;
    let game_mods = game.join("mods");
    if game_mods.exists() {
        fs::remove_dir_all(&game_mods).map_err(|e| e.to_string())?;
    }
    let src_mods = src.join("mods");
    if src_mods.exists() {
        copy_dir(&src_mods, &game_mods).map_err(|e| e.to_string())?;
    } else {
        fs::create_dir_all(&game_mods).map_err(|e| e.to_string())?;
    }
    let dll = src.join("version.dll");
    if dll.exists() {
        fs::copy(&dll, game.join("version.dll")).map_err(|e| e.to_string())?;
    }
    log_file("app.log", &format!("restored {name}"));
    Ok(())
}

// --- profile: welche mods an sind, mit einem namen ---
#[derive(Serialize, Deserialize, Default)]
struct ProfileFile {
    sets: BTreeMap<String, Vec<String>>,
}

fn load_profiles() -> ProfileFile {
    data_path("profiles.json")
        .and_then(|p| read_json(&p))
        .unwrap_or_default()
}

fn save_profiles(file: &ProfileFile) -> Result<(), String> {
    write_json(&data_path("profiles.json").ok_or("could not find the local app data folder")?, file)
}

#[derive(Serialize)]
pub struct ProfileRow {
    name: String,
    mods: Vec<String>,
}

#[tauri::command]
pub fn list_profiles() -> Vec<ProfileRow> {
    load_profiles()
        .sets
        .into_iter()
        .map(|(name, mods)| ProfileRow { name, mods })
        .collect()
}

#[tauri::command]
pub fn save_profile(name: String) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 40 {
        return Err("Profile name must be 1 to 40 characters".into());
    }
    let game = find_game().ok_or("Horsey Game was not found")?;
    let mut file = load_profiles();
    file.sets.insert(name, enabled_folders(&game));
    save_profiles(&file)
}

#[tauri::command]
pub fn apply_profile(name: String) -> Result<(), String> {
    if horsey_running() {
        return Err("Close Horsey first".into());
    }
    guard_mods()?;
    let wanted = load_profiles()
        .sets
        .get(&name)
        .ok_or("No such profile")?
        .clone();
    let game = find_game().ok_or("Horsey Game was not found")?;
    for dir in mod_dirs(&game) {
        let folder = folder_name(&dir);
        set_mod_enabled(&game, &folder, wanted.contains(&folder))?;
    }
    log_file("app.log", &format!("profile {name} applied"));
    Ok(())
}

#[tauri::command]
pub fn delete_profile(name: String) -> Result<(), String> {
    let mut file = load_profiles();
    file.sets.remove(&name).ok_or("No such profile")?;
    save_profiles(&file)
}

// --- spielversion: hash von Horsey.exe, merken und bei änderung warnen ---
#[derive(Serialize, Deserialize)]
struct GameRecord {
    sha256: String,
    size: u64,
    modified: u64,
}

#[derive(Serialize)]
pub struct GameVersion {
    known: bool,
    changed: bool,
    sha256: String,
    size: u64,
}

fn exe_stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((meta.len(), modified))
}

#[tauri::command]
pub fn game_version_status() -> Result<GameVersion, String> {
    let game = find_game().ok_or("Horsey Game was not found")?;
    let exe = game.join("Horsey.exe");
    let (size, modified) = exe_stamp(&exe).ok_or("could not read Horsey.exe")?;
    let Some(record) = data_path("game_version.json").and_then(|p| read_json::<GameRecord>(&p)) else {
        return Ok(GameVersion {
            known: false,
            changed: false,
            sha256: String::new(),
            size,
        });
    };
    // gleiche größe und zeit: nicht neu hashen
    if record.size == size && record.modified == modified {
        return Ok(GameVersion {
            known: true,
            changed: false,
            sha256: record.sha256,
            size,
        });
    }
    let hash = file_sha(&exe).ok_or("could not read Horsey.exe")?;
    Ok(GameVersion {
        known: true,
        changed: hash != record.sha256,
        sha256: hash,
        size,
    })
}

#[tauri::command]
pub fn accept_game_version() -> Result<(), String> {
    let game = find_game().ok_or("Horsey Game was not found")?;
    let exe = game.join("Horsey.exe");
    let (size, modified) = exe_stamp(&exe).ok_or("could not read Horsey.exe")?;
    let hash = file_sha(&exe).ok_or("could not read Horsey.exe")?;
    write_json(
        &data_path("game_version.json").ok_or("could not find the local app data folder")?,
        &GameRecord {
            sha256: hash,
            size,
            modified,
        },
    )
}

// --- shim-signaturen: dieselben muster wie check_sig.py, ? ist ein beliebiges byte ---
const SHIM_SIGS: &[(&str, &str)] = &[
    ("game_frame", "40 53 48 83 EC 40 0F 29 74 24 30 0F 29 7C 24 20 E8 ? ? ? ? 80 3D ? ? ? ? 00 0F 57 C0 F3 0F 2A 05 ? ? ? ?"),
    ("player_add_money", "48 83 EC 58 89 91 10 03 00 00 01 91 08 03 00 00 C7 81 0C 03 00 00 3C 00 00 00 45 84 C0 0F 85"),
    ("spend_money", "40 53 48 83 EC 50 44 8B 91 08 03 00 00 44 3B D2 0F 8C ? ? ? ? 8B C2 F7 D8 89 81 10 03 00 00 44 2B D2 44 89 91 08 03 00 00 C7 81 0C 03 00 00"),
    ("horse_records_ref", "48 83 EC 28 83 F9 FF 75 18 48 8D 0D ? ? ? ? E8 ? ? ? ? ? ? ? ? ? ? ? ? ? ? ? ? 48 63 C1 48 69 C0 88 00 00 00 48 03 05 ? ? ? ?"),
    ("world_obj_ref", "48 8B 0D ? ? ? ? 42 80 7C 07 09 00 74 12 45 33 C0 BA FA 00 00 00 E8"),
    ("world_spawn_herd", "48 89 5C 24 08 4C 89 4C 24 20 55 56 57 41 54 41 55 41 56 41 57 48 83 EC 30 49 8B F9 41 8B E8 8B DA 4C 8B F9 48 8B B4 24 A0 00 00 00 44 8B 94 24 90 00 00 00 45 85 D2 0F 8E"),
    ("pop_map_ref", "48 89 5C 24 18 56 57 41 57 48 81 EC 80 00 00 00 48 89 AC 24 A0 00 00 00 48 8B F2 48 8B 2D ? ? ? ? 4C 8B F9 48 8B DD 48 8B 7D 08 80 7F 19 00"),
];

fn parse_pattern(pat: &str) -> Vec<Option<u8>> {
    pat.split_whitespace()
        .map(|t| {
            if t == "?" {
                None
            } else {
                Some(u8::from_str_radix(t, 16).expect("bad signature byte"))
            }
        })
        .collect()
}

fn count_matches(data: &[u8], pat: &[Option<u8>]) -> usize {
    if pat.is_empty() || data.len() < pat.len() {
        return 0;
    }
    (0..=data.len() - pat.len())
        .filter(|&i| {
            pat.iter().enumerate().all(|(j, p)| match p {
                Some(b) => data[i + j] == *b,
                None => true,
            })
        })
        .count()
}

#[derive(Serialize)]
pub struct SigRow {
    name: String,
    matches: usize,
    ok: bool,
}

#[tauri::command]
pub fn shim_signatures() -> Result<Vec<SigRow>, String> {
    let game = find_game().ok_or("Horsey Game was not found")?;
    let data = fs::read(game.join("Horsey.exe")).map_err(|e| e.to_string())?;
    Ok(SHIM_SIGS
        .iter()
        .map(|(name, pat)| {
            let matches = count_matches(&data, &parse_pattern(pat));
            SigRow {
                name: name.to_string(),
                matches,
                ok: matches == 1,
            }
        })
        .collect())
}

// --- safe mode: alle mods aus, nach dem schließen wieder an ---
// merkt sich, welche mods an waren; nach einem absturz des managers wird beim start aufgeräumt
fn safe_mode_begin(game: &Path) -> Result<(), String> {
    let path = safe_path().ok_or("could not find the local app data folder")?;
    if path.exists() {
        return Err("Safe mode is already active".into());
    }
    let enabled = enabled_folders(game);
    write_json(&path, &enabled)?;
    for folder in &enabled {
        if let Err(e) = set_mod_enabled(game, folder, false) {
            safe_mode_end(game);
            return Err(e);
        }
    }
    Ok(())
}

fn safe_mode_end(game: &Path) {
    let Some(path) = safe_path() else {
        return;
    };
    if !path.exists() {
        return;
    }
    if let Some(enabled) = read_json::<Vec<String>>(&path) {
        for folder in &enabled {
            let _ = set_mod_enabled(game, folder, true);
        }
    }
    let _ = fs::remove_file(&path);
    log_file("app.log", "safe mode ended, mods restored");
}

// absturz oder normales ende: exit-code nur, wenn der manager selbst gestartet hat
fn report_exit(game: &Path, offset: u64, code: Option<i32>, exit_known: bool) -> CrashReport {
    let mut report = crash_report(game, offset, code);
    report.exit_known = exit_known;
    if !exit_known {
        report.crashed = false;
    }
    record_crash(&report);
    report
}

// --- absturzverlauf: die letzten 100 enden, damit wiederholte fehler auffallen ---
#[derive(Serialize, Deserialize, Clone)]
pub struct HistoryEntry {
    time: u64,
    code: Option<i32>,
    crashed: bool,
    exit_known: bool,
    culprit: Option<String>,
    line: Option<String>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn record_crash(report: &CrashReport) {
    let Some(path) = data_path("crashes.json") else {
        return;
    };
    let mut list: Vec<HistoryEntry> = read_json(&path).unwrap_or_default();
    list.push(HistoryEntry {
        time: now_secs(),
        code: report.code,
        crashed: report.crashed,
        exit_known: report.exit_known,
        culprit: report.culprit.clone(),
        line: report.line.clone(),
    });
    if list.len() > 100 {
        let extra = list.len() - 100;
        list.drain(..extra);
    }
    let _ = write_json(&path, &list);
}

#[tauri::command]
pub fn crash_history() -> Vec<HistoryEntry> {
    data_path("crashes.json")
        .and_then(|p| read_json(&p))
        .unwrap_or_default()
}

#[tauri::command]
pub fn clear_crash_history() -> Result<(), String> {
    let path = data_path("crashes.json").ok_or("could not find the local app data folder")?;
    if path.exists() {
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// --- log: die letzten zeilen von SHIM.log, optional nur von einem mod ---
#[tauri::command]
pub fn shim_log(lines: usize, mod_id: Option<String>) -> Result<Vec<String>, String> {
    use std::io::{Read, Seek, SeekFrom};
    let lines = lines.clamp(1, 2000);
    let game = find_game().ok_or("Horsey Game was not found")?;
    let mut file = fs::File::open(game.join("SHIM.log")).map_err(|e| e.to_string())?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    // nur das ende lesen, eine zeile ist grob 400 bytes
    let window = (lines as u64).saturating_mul(400).clamp(64 * 1024, 16 * 1024 * 1024);
    let start = len.saturating_sub(window);
    file.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let mut all: Vec<&str> = text.lines().collect();
    // die erste zeile kann mitten drin anfangen
    if start > 0 && !all.is_empty() {
        all.remove(0);
    }
    let tag = mod_id.filter(|id| !id.is_empty()).map(|id| format!("[{id}]"));
    let kept: Vec<String> = all
        .into_iter()
        .filter(|l| tag.as_ref().map_or(true, |t| l.starts_with(t.as_str())))
        .map(String::from)
        .collect();
    let skip = kept.len().saturating_sub(lines);
    Ok(kept.into_iter().skip(skip).collect())
}

// --- config-bereich aus dem manifest: min und max für zahlen prüfen ---
pub fn check_range(dir: &Path, field: &ConfigField) -> Result<(), String> {
    if field.kind != "number" {
        return Ok(());
    }
    let schema = read_manifest(dir)
        .and_then(|m| m["config_schema"].as_array().cloned())
        .unwrap_or_default();
    let Some(entry) = schema.iter().find(|e| e["key"].as_str() == Some(field.key.as_str())) else {
        return Ok(());
    };
    let value: f64 = field
        .value
        .trim()
        .parse()
        .map_err(|_| format!("{} must be a number", field.key))?;
    let label = entry["label"].as_str().unwrap_or(&field.key);
    if let Some(min) = entry["min"].as_f64() {
        if value < min {
            return Err(format!("{label} must be at least {min}"));
        }
    }
    if let Some(max) = entry["max"].as_f64() {
        if value > max {
            return Err(format!("{label} must be at most {max}"));
        }
    }
    Ok(())
}

// startet horsey und meldet das ende. safe = mods vorher aus, danach wieder an
pub fn launch(app: AppHandle, safe: bool) -> Result<(), String> {
    if horsey_running() {
        return Err("Horsey is already running".into());
    }
    let game = find_game().ok_or("Horsey Game was not found")?;
    if safe {
        guard_mods()?;
        safe_mode_begin(&game)?;
    }
    let offset = fs::metadata(game.join("SHIM.log")).map(|m| m.len()).unwrap_or(0);
    LAUNCH_ACTIVE.store(true, Ordering::SeqCst);
    let spawned = std::process::Command::new(game.join("Horsey.exe"))
        .current_dir(&game)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            LAUNCH_ACTIVE.store(false, Ordering::SeqCst);
            if safe {
                safe_mode_end(&game);
            }
            return Err(e.to_string());
        }
    };
    log_file(
        "app.log",
        if safe { "horsey launched in safe mode" } else { "horsey launched" },
    );
    // auf das ende warten, ohne die oberfläche zu blockieren
    std::thread::spawn(move || {
        let code = child.wait().ok().and_then(|s| s.code());
        if safe {
            safe_mode_end(&game);
        }
        let report = report_exit(&game, offset, code, true);
        LAUNCH_ACTIVE.store(false, Ordering::SeqCst);
        log_file(
            "app.log",
            &format!("horsey closed, code {code:?}, culprit {:?}", report.culprit),
        );
        let _ = app.emit("horsey-exited", report);
    });
    Ok(())
}

#[tauri::command]
pub fn safe_mode_launch(app: AppHandle) -> Result<(), String> {
    launch(app, true)
}

// sieht horsey, das nicht aus dem manager gestartet wurde, und meldet dessen ende (ohne exit-code)
pub fn start_watch(app: AppHandle) {
    std::thread::spawn(move || {
        let mut session: Option<(PathBuf, u64)> = None;
        loop {
            if horsey_running() {
                if session.is_none() && !LAUNCH_ACTIVE.load(Ordering::SeqCst) {
                    if let Some(game) = find_game() {
                        let offset = fs::metadata(game.join("SHIM.log")).map(|m| m.len()).unwrap_or(0);
                        session = Some((game, offset));
                    }
                }
            } else {
                // safe mode, die der manager nicht mehr beenden konnte, jetzt aufräumen
                if safe_path().is_some_and(|p| p.exists()) {
                    if let Some(game) = find_game() {
                        safe_mode_end(&game);
                    }
                }
                if let Some((game, offset)) = session.take() {
                    let report = report_exit(&game, offset, None, false);
                    log_file("app.log", "horsey closed (started outside the manager)");
                    let _ = app.emit("horsey-exited", report);
                }
            }
            std::thread::sleep(Duration::from_secs(3));
        }
    });
}

// --- load order: mods/load_order.txt, eine zeile pro ordner, oben lädt zuerst ---
// der shim liest die datei erst, wenn er sie unterstützt. bis dahin wird sie ignoriert.
fn load_order_path(game: &Path) -> PathBuf {
    game.join("mods").join("load_order.txt")
}

#[tauri::command]
pub fn load_order() -> Result<Vec<String>, String> {
    let game = find_game().ok_or("Horsey Game was not found")?;
    let mut rest: Vec<String> = mod_dirs(&game).iter().map(|d| folder_name(d)).collect();
    rest.sort_by_key(|s| s.to_lowercase());
    let mut ordered = Vec::new();
    if let Ok(text) = fs::read_to_string(load_order_path(&game)) {
        for line in text.lines() {
            let name = line.trim();
            if let Some(pos) = rest.iter().position(|r| r == name) {
                ordered.push(rest.remove(pos));
            }
        }
    }
    // was nicht in der datei steht, kommt hinten dran
    ordered.extend(rest);
    Ok(ordered)
}

#[tauri::command]
pub fn save_load_order(order: Vec<String>) -> Result<(), String> {
    let game = find_game().ok_or("Horsey Game was not found")?;
    let known: Vec<String> = mod_dirs(&game).iter().map(|d| folder_name(d)).collect();
    let mut seen: Vec<&String> = Vec::new();
    for name in &order {
        if !known.contains(name) {
            return Err(format!("{name} is not in the mods folder"));
        }
        if seen.contains(&name) {
            return Err(format!("{name} is listed twice"));
        }
        seen.push(name);
    }
    let text: String = order.iter().map(|n| format!("{n}\n")).collect();
    fs::write(load_order_path(&game), text).map_err(|e| e.to_string())?;
    log_file("app.log", "load order saved");
    Ok(())
}
