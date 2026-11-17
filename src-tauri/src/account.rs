// konto, uploads und entwürfe. übernommen aus dem alten HorseyModManager.
// der server ist mods.badasswizard.boats, HORSEY_SERVER geht nur für lokale tests (localhost).

use serde::Serialize;
use std::os::windows::process::CommandExt;
use std::{
    fs,
    path::{Path, PathBuf},
};

use super::http;

const MOD_SERVER: &str = "https://horsey-web.badasswizard.boats";
const TOKEN_SERVICE: &str = "HorseyModManager";
const TOKEN_USER: &str = "account";
const MAX_UPLOAD_BYTES: u64 = 50 * 1024 * 1024;
const MAX_UPLOAD_FILES: usize = 500;
const MAX_ICON_BYTES: u64 = 256 * 1024;
const ICON_MAX_SIDE: u32 = 512;
const RESERVED_IDS: [&str; 5] = ["mapview", "autonamer", "shim", "dumphorse", "example"];

// normalerweise der live server. mit dem flag -test geht es an localhost:5000.
// HORSEY_SERVER darf nur auf localhost zeigen (zum testen).
fn server_base() -> Result<String, String> {
    if std::env::args().any(|a| a == "-test") {
        return Ok("http://localhost:5000".to_string());
    }
    match std::env::var("HORSEY_SERVER") {
        Ok(url) if is_local_url(&url) => Ok(url.trim_end_matches('/').to_string()),
        Ok(_) => Err("HORSEY_SERVER must point at localhost for testing".into()),
        Err(_) => Ok(MOD_SERVER.to_string()),
    }
}

// ganz genau prüfen: schema, host, kein benutzername. "localhost:5000@evil" besteht sonst.
fn is_local_url(url: &str) -> bool {
    match reqwest::Url::parse(url) {
        Ok(u) => {
            u.scheme() == "http"
                && matches!(u.host_str(), Some("localhost") | Some("127.0.0.1"))
                && u.username().is_empty()
                && u.password().is_none()
        }
        Err(_) => false,
    }
}

// -test: die anmeldung gilt nur für diesen lauf und wird nirgends gespeichert.
// ohne -test bleibt man über neustarts angemeldet (windows-anmeldeinformationen).
static TEST_TOKEN: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn is_test() -> bool {
    std::env::args().any(|a| a == "-test")
}

fn token_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(TOKEN_SERVICE, TOKEN_USER).map_err(|e| e.to_string())
}

fn saved_token() -> Option<String> {
    if is_test() {
        return TEST_TOKEN.lock().ok()?.clone();
    }
    token_entry().ok()?.get_password().ok()
}

fn save_token(token: &str) -> Result<(), String> {
    if is_test() {
        *TEST_TOKEN.lock().map_err(|e| e.to_string())? = Some(token.to_string());
        return Ok(());
    }
    token_entry()?.set_password(token).map_err(|e| e.to_string())
}

fn forget_token() {
    if is_test() {
        if let Ok(mut token) = TEST_TOKEN.lock() {
            *token = None;
        }
        return;
    }
    if let Ok(entry) = token_entry() {
        let _ = entry.delete_credential();
    }
}

// der anzeigename wird lokal gemerkt, damit die anzeige stimmt, auch wenn der server gerade nicht antwortet
fn cached_name_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var("LOCALAPPDATA").ok()?).join("HorseyModManager").join("account.json"))
}

fn remember_name(name: &str) {
    if is_test() {
        return;
    }
    if let Some(path) = cached_name_path() {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(path, serde_json::json!({ "username": name }).to_string());
    }
}

fn cached_name() -> Option<String> {
    if is_test() {
        return None;
    }
    let text = fs::read_to_string(cached_name_path()?).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json["username"].as_str().map(str::to_string)
}

fn forget_cached_name() {
    if is_test() {
        return;
    }
    if let Some(path) = cached_name_path() {
        let _ = fs::remove_file(path);
    }
}

#[derive(Serialize)]
pub struct Account {
    username: String,
}

async fn server_json(resp: reqwest::Response) -> Result<serde_json::Value, String> {
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    if status == reqwest::StatusCode::UNAUTHORIZED {
        forget_token();
    }
    if body["code"].as_str() == Some("maintenance") {
        return Err(format!(
            "under maintenance: {}",
            body["error"].as_str().unwrap_or("the mod server is down for a while, try again later")
        ));
    }
    if status.is_success() {
        return Ok(body);
    }
    Err(body["error"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("the server said {status}")))
}

fn open_browser(url: &str) -> Result<(), String> {
    // rundll32 bekommt die url als ein argument, also kein shell-parsing von & und ?
    std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .creation_flags(0x0800_0000)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open your browser: {e}"))
}

fn parse_callback(line: &str, state: &str) -> Result<String, String> {
    let target = line.split_whitespace().nth(1).unwrap_or("");
    let query = target
        .strip_prefix("/callback?")
        .ok_or_else(|| "This is not a sign-in response.".to_string())?;
    let mut code = None;
    let mut given = None;
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix("code=") {
            code = Some(v);
        }
        if let Some(v) = pair.strip_prefix("state=") {
            given = Some(v);
        }
    }
    if given != Some(state) {
        return Err("This sign-in did not come from Horsey Mod Manager.".into());
    }
    code.filter(|c| !c.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "No sign-in code was sent.".into())
}

// wartet auf 127.0.0.1, bis der browser mit dem einmal-code zurückkommt
fn wait_for_callback(listener: std::net::TcpListener, state: &str) -> Result<String, String> {
    use std::io::{ErrorKind, Read, Write};
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                let result = parse_callback(request.lines().next().unwrap_or(""), state);
                let message = match &result {
                    Ok(_) => "Signed in. You can close this tab and go back to Horsey Mod Manager.",
                    Err(e) => e.as_str(),
                };
                let page = format!(
                    "<!doctype html><meta charset=\"utf-8\"><title>Horsey Mod Manager</title><p>{message}</p>"
                );
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                    page.len(),
                    page
                );
                if let Ok(code) = result {
                    return Ok(code);
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("sign-in timed out, try again".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

#[tauri::command]
pub async fn sign_in() -> Result<Account, String> {
    let base = server_base()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let state: String = (0..32).map(|_| format!("{:02x}", rand::random::<u8>())).collect();
    open_browser(&format!("{base}/app/login?port={port}&state={state}"))?;
    let expected = state.clone();
    let code = tauri::async_runtime::spawn_blocking(move || wait_for_callback(listener, &expected))
        .await
        .map_err(|e| e.to_string())??;
    let resp = http()
        .post(format!("{base}/api/token"))
        .json(&serde_json::json!({ "code": code }))
        .send()
        .await
        .map_err(|e| format!("could not reach the mod server: {e}"))?;
    let body = server_json(resp).await?;
    let token = body["token"].as_str().ok_or("the server sent no token")?;
    save_token(token)?;
    let username = body["username"].as_str().unwrap_or("").to_string();
    remember_name(&username);
    Ok(Account { username })
}

#[tauri::command]
pub async fn account() -> Result<Option<Account>, String> {
    let Some(token) = saved_token() else {
        return Ok(None);
    };
    let resp = match http()
        .get(format!("{}/api/me", server_base()?))
        .bearer_auth(token)
        .send()
        .await
    {
        Ok(resp) => resp,
        // server nicht erreichbar: die anmeldung bleibt, mit dem letzten bekannten namen
        Err(_) => return Ok(cached_name().map(|username| Account { username })),
    };
    match server_json(resp).await {
        Ok(body) => {
            let username = body["display_name"].as_str().unwrap_or("").to_string();
            remember_name(&username);
            Ok(Some(Account { username }))
        }
        Err(_) => Ok(None),
    }
}

#[tauri::command]
pub async fn sign_out() -> Result<(), String> {
    if let (Some(token), Ok(base)) = (saved_token(), server_base()) {
        let _ = http().post(format!("{base}/api/logout")).bearer_auth(token).send().await;
    }
    forget_token();
    forget_cached_name();
    Ok(())
}

#[tauri::command]
pub async fn my_uploads() -> Result<serde_json::Value, String> {
    let token = saved_token().ok_or("sign in first")?;
    let resp = http()
        .get(format!("{}/api/my-uploads", server_base()?))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("could not reach the mod server: {e}"))?;
    server_json(resp).await
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            return Err(format!("links are not allowed in a mod folder: {}", path.display()));
        }
        if kind.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            let rel = path.strip_prefix(root).map_err(|e| e.to_string())?;
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel, path));
            if out.len() > MAX_UPLOAD_FILES {
                return Err("too many files in the mod".into());
            }
        }
    }
    Ok(())
}

// eine zip wird so gesendet, ein ordner wird mit dem ordnernamen als oberste ebene gezippt
fn package_for_upload(path: &Path) -> Result<Vec<u8>, String> {
    if path.is_file() {
        let meta = fs::metadata(path).map_err(|e| e.to_string())?;
        if meta.len() > MAX_UPLOAD_BYTES {
            return Err("zip is larger than 50 MB".into());
        }
        return fs::read(path).map_err(|e| e.to_string());
    }
    let root_name = path
        .file_name()
        .ok_or("that folder has no name")?
        .to_string_lossy()
        .into_owned();
    let mut files = Vec::new();
    collect_files(path, path, &mut files)?;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (rel, abs) in &files {
        // der entwurf heißt manifest.json.disabled, im zip muss es manifest.json heißen
        let name = if rel == "manifest.json.disabled" { "manifest.json" } else { rel.as_str() };
        zip.start_file(format!("{root_name}/{name}"), options)
            .map_err(|e| e.to_string())?;
        let data = fs::read(abs).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut zip, &data).map_err(|e| e.to_string())?;
    }
    let bytes = zip.finish().map_err(|e| e.to_string())?.into_inner();
    if bytes.len() as u64 > MAX_UPLOAD_BYTES {
        return Err("mod is larger than 50 MB".into());
    }
    Ok(bytes)
}

#[tauri::command]
pub async fn upload_mod(path: String) -> Result<serde_json::Value, String> {
    let token = saved_token().ok_or("sign in first")?;
    let base = server_base()?;
    let bytes = package_for_upload(Path::new(&path))?;
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name("mod.zip")
        .mime_str("application/zip")
        .map_err(|e| e.to_string())?;
    let form = reqwest::multipart::Form::new().part("file", part);
    let resp = http()
        .post(format!("{base}/api/upload"))
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("could not reach the mod server: {e}"))?;
    server_json(resp).await
}

// entwürfe liegen im mods-ordner des spiels. die manifest-datei heißt .disabled,
// damit das spiel den entwurf nicht lädt, solange er nicht fertig ist.
fn drafts_root() -> Result<PathBuf, String> {
    let game = super::find_game().ok_or("Horsey Game was not found")?;
    Ok(game.join("mods"))
}

#[derive(Serialize)]
pub struct Draft {
    id: String,
    name: String,
    version: String,
    dir: String,
}

#[tauri::command]
pub fn list_drafts() -> Vec<Draft> {
    let Ok(root) = drafts_root() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out: Vec<Draft> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter_map(|dir| {
            let text = fs::read_to_string(dir.join("manifest.json.disabled"))
                .or_else(|_| fs::read_to_string(dir.join("manifest.json")))
                .ok()?;
            let json: serde_json::Value = serde_json::from_str(&text).ok()?;
            // nur ordner mit dem draft-merkmal sind entwürfe, der rest sind normale mods
            if json["draft"].as_bool() != Some(true) {
                return None;
            }
            Some(Draft {
                id: json["id"].as_str()?.to_string(),
                name: json["name"].as_str().unwrap_or("").to_string(),
                version: json["version"].as_str().unwrap_or("").to_string(),
                dir: dir.display().to_string(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[derive(serde::Deserialize)]
pub struct NewMod {
    id: String,
    name: String,
    version: String,
    description: String,
    api_version: i64,
    icon: String,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && !RESERVED_IDS.contains(&id)
}

fn check_png(data: &[u8]) -> Result<(), String> {
    if data.len() < 24 || &data[..8] != b"\x89PNG\r\n\x1a\n" || &data[12..16] != b"IHDR" {
        return Err("the icon must be a PNG file".into());
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    if width == 0 || height == 0 || width > ICON_MAX_SIDE || height > ICON_MAX_SIDE {
        return Err(format!("the icon must be at most {ICON_MAX_SIDE} pixels on each side"));
    }
    Ok(())
}

// startdatei für einen neuen mod. die api steht in API.md im horsey-shim repo.
fn main_lua_template(id: &str) -> String {
    format!(
        "-- {id}\n\
         -- this file runs inside the game through the shim.\n\
         -- the functions are listed in API.md in the horsey-shim repo.\n\n\
         shim.on(\"ready\", function()\n    shim.log(\"{id} is running\")\nend)\n"
    )
}

#[tauri::command]
pub fn create_mod(form: NewMod) -> Result<Draft, String> {
    let id = form.id.trim().to_string();
    let name = form.name.trim().to_string();
    let version = form.version.trim().to_string();
    let description = form.description.trim().to_string();
    if !valid_id(&id) {
        return Err("id must be 1 to 40 lowercase letters, numbers, - or _, and not a reserved name".into());
    }
    if name.is_empty() || name.chars().count() > 60 {
        return Err("name must be 1 to 60 characters".into());
    }
    if version.is_empty()
        || version.len() > 32
        || !version.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    {
        return Err("version must be 1 to 32 letters, numbers, dots, - or _".into());
    }
    if description.chars().count() > 500 {
        return Err("description must be at most 500 characters".into());
    }
    if !(1..=2).contains(&form.api_version) {
        return Err("api_version must be 1 or 2".into());
    }
    let icon = match form.icon.trim() {
        "" => None,
        path => {
            let path = PathBuf::from(path);
            let meta = fs::metadata(&path).map_err(|e| format!("could not read the icon: {e}"))?;
            if meta.len() > MAX_ICON_BYTES {
                return Err("the icon is larger than 256 KB".into());
            }
            let data = fs::read(&path).map_err(|e| e.to_string())?;
            check_png(&data)?;
            Some(data)
        }
    };
    let dir = drafts_root()?.join(&id);
    if dir.exists() {
        return Err(format!("there is already a mod folder called {id}"));
    }
    let manifest = serde_json::json!({
        "id": id,
        "name": name,
        "version": version,
        "description": description,
        "api_version": form.api_version,
        "draft": true,
    });
    let write = || -> Result<(), String> {
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
        fs::write(dir.join("manifest.json.disabled"), text).map_err(|e| e.to_string())?;
        fs::write(dir.join("main.lua"), main_lua_template(&id)).map_err(|e| e.to_string())?;
        if let Some(data) = &icon {
            fs::write(dir.join("icon.png"), data).map_err(|e| e.to_string())?;
        }
        Ok(())
    };
    if let Err(e) = write() {
        let _ = fs::remove_dir_all(&dir);
        return Err(e);
    }
    Ok(Draft {
        id,
        name,
        version,
        dir: dir.display().to_string(),
    })
}


// meldet einen installierten mod beim server. fehler werden verschluckt, die installation bleibt gültig.
// ohne anmeldung passiert nichts. der server zählt pro benutzer nur einmal.
pub async fn report_download(mod_id: &str) {
    let Some(token) = saved_token() else {
        return;
    };
    let Ok(base) = server_base() else {
        return;
    };
    let _ = http()
        .post(format!("{base}/api/mods/{mod_id}/download"))
        .bearer_auth(token)
        .send()
        .await;
}

// die id kommt in den url-pfad, also nur die zeichen, die ein mod-id haben darf
fn safe_mod_id(id: &str) -> Result<(), String> {
    if !id.is_empty()
        && id.len() <= 40
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        Ok(())
    } else {
        Err("that is not a valid mod id".into())
    }
}

// zählt die downloads eines mods (pro benutzer einmal). öffentlich, ohne anmeldung.
#[tauri::command]
pub async fn mod_downloads(id: String) -> Result<u64, String> {
    safe_mod_id(&id)?;
    let base = server_base()?;
    let resp = http()
        .get(format!("{base}/api/mods/{id}/downloads"))
        .send()
        .await
        .map_err(|e| format!("could not reach the mod server: {e}"))?;
    let body = server_json(resp).await?;
    Ok(body["downloads"].as_u64().unwrap_or(0))
}

// meldet einen mod. der server legt die meldung in die prüfwarteschlange.
#[tauri::command]
pub async fn report_mod(id: String, reason: String) -> Result<(), String> {
    safe_mod_id(&id)?;
    let token = saved_token().ok_or("sign in first")?;
    let base = server_base()?;
    let reason = reason.trim().to_string();
    if reason.is_empty() || reason.chars().count() > 500 {
        return Err("reason must be 1 to 500 characters".into());
    }
    let resp = http()
        .post(format!("{base}/api/mods/{id}/report"))
        .bearer_auth(token)
        .json(&serde_json::json!({ "reason": reason }))
        .send()
        .await
        .map_err(|e| format!("could not reach the mod server: {e}"))?;
    server_json(resp).await.map(|_| ())
}
