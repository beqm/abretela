//! Servidor próprio (ROADMAP, seção 6, fase S2): o app monta, atualiza e apaga
//! o Worker da sala na conta da Cloudflare da pessoa, só com um token da API.
//! O código do Worker (`server/`) vai embutido no app (`npm run build` o
//! empacota em `server/dist/worker.js`).
//!
//! Onde fica cada coisa: o token e a chave de administrador no Gerenciador de
//! Credenciais (`secrets.rs`); a chave do TURN só nos segredos do Worker; o
//! resto (nome, endereço, conta) nas preferências do app, pelo frontend.

mod cloudflare;
mod secrets;

use cloudflare::{Account, CfError, Cloudflare};
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// O Worker, empacotado pelo `scripts/build-server.mjs`.
const WORKER_JS: &str = include_str!("../../../server/dist/worker.js");
/// A mesma de `server/wrangler.jsonc`.
const COMPATIBILITY_DATE: &str = "2026-11-01";
const SCRIPT_PREFIX: &str = "abretela-";

/// Um servidor deste dono, como o frontend guarda nas preferências.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedServer {
    script_name: String,
    account_id: String,
    name: String,
    url: String,
    /// Chave do TURN criada para este servidor (para apagar junto).
    turn_key_id: Option<String>,
    /// Por que ficou sem TURN, se ficou.
    turn_error: Option<String>,
    /// A senha da sala já foi definida? (O Worker pode demorar a responder.)
    configured: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundServer {
    script_name: String,
    url: String,
}

fn random_hex(bytes: usize) -> String {
    use ring::rand::SecureRandom;
    let mut buf = vec![0u8; bytes];
    ring::rand::SystemRandom::new().fill(&mut buf).expect("random numbers");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_hex(text: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, text.as_bytes()).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn worker_url(script: &str, subdomain: &str) -> String {
    format!("https://{script}.{subdomain}.workers.dev")
}

/// Metadados do upload. Na primeira vez cria a classe da sala (SQLite, a única
/// do plano grátis) e põe os segredos; nas outras mantém os segredos que já estão lá.
fn metadata(first: bool, secrets: &[(&str, &str)]) -> Value {
    let mut bindings = vec![json!({ "type": "durable_object_namespace", "name": "ROOM", "class_name": "Room" })];
    for (name, text) in secrets {
        bindings.push(json!({ "type": "secret_text", "name": name, "text": text }));
    }
    let mut m = json!({ "main_module": "worker.js", "compatibility_date": COMPATIBILITY_DATE, "bindings": bindings });
    if first {
        // o mesmo formato que o wrangler manda
        m["migrations"] = json!({ "new_tag": "v1", "steps": [{ "new_sqlite_classes": ["Room"] }] });
    } else {
        m["keep_bindings"] = json!(["secret_text", "secret_key"]);
    }
    m
}

fn saved_client() -> Result<Cloudflare, String> {
    let token = secrets::token()?.ok_or("no Cloudflare token saved on this PC")?;
    Cloudflare::new(&token).map_err(|e| e.to_string())
}

fn progress(app: &AppHandle, step: &str) {
    let _ = app.emit("selfhost-progress", step);
}

fn check_room_fields(name: &str, password: &str) -> Result<(String, String), String> {
    let (name, password) = (name.trim(), password.trim());
    if name.is_empty() || name.chars().count() > 40 {
        return Err("the server name should have 1 to 40 characters".into());
    }
    if password.is_empty() || password.chars().count() > 64 {
        return Err("the room password should have 1 to 64 characters".into());
    }
    Ok((name.to_string(), password.to_string()))
}

fn plain_client(timeout: u64) -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(timeout)).build().expect("HTTP client")
}

/// O Worker novo já responde? (Um `workers.dev` novo pode levar um tempo.)
async fn wait_online(url: &str, max: Duration) -> bool {
    let client = plain_client(10);
    let deadline = std::time::Instant::now() + max;
    while std::time::Instant::now() < deadline {
        if let Ok(r) = client.get(format!("{url}/api/info")).send().await {
            if r.status().is_success() && r.json::<Value>().await.is_ok_and(|v| v["app"] == "abretela") {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    false
}

/// Define o nome e a senha da sala pela administração do próprio Worker.
async fn configure_room(url: &str, admin_key: &str, name: &str, password: &str) -> Result<(), String> {
    let r = plain_client(20)
        .post(format!("{url}/api/admin/room"))
        .bearer_auth(admin_key)
        .json(&json!({ "name": name, "password": password }))
        .send()
        .await
        .map_err(|e| format!("could not reach the new server: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("the new server refused the settings (HTTP {})", r.status().as_u16()));
    }
    Ok(())
}

async fn subdomain_or_create(cf: &Cloudflare, account: &str) -> Result<String, String> {
    if let Some(s) = cf.subdomain(account).await.map_err(|e| e.to_string())? {
        return Ok(s);
    }
    // conta sem workers.dev ainda: cria um (o nome pode estar ocupado)
    let mut last = String::new();
    for _ in 0..3 {
        match cf.create_subdomain(account, &format!("{SCRIPT_PREFIX}{}", random_hex(4))).await {
            Ok(s) => {
                log_info!("selfhost", "Created the account's workers.dev address ({s}.workers.dev)");
                return Ok(s);
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(format!("could not create the account's workers.dev address: {last}"))
}

/// Desfaz a chave do TURN criada para um servidor que não chegou a existir.
async fn undo_turn(cf: &Cloudflare, account: &str, uid: Option<&str>) {
    if let Some(uid) = uid {
        let _ = cf.delete_turn_key(account, uid).await;
    }
}

fn turn_permission_hint(e: &CfError) -> String {
    if e.is_forbidden() {
        "the token has no Realtime permission".into()
    } else {
        format!("could not create the relay server (TURN) key: {e}")
    }
}

// ---------- comandos ----------

/// Confere o token, guarda no Gerenciador de Credenciais e devolve as contas.
#[tauri::command]
pub async fn selfhost_connect(token: String) -> Result<Vec<Account>, String> {
    let cf = Cloudflare::new(&token).map_err(|e| e.to_string())?;
    let accounts = cf.accounts().await.map_err(|e| {
        if e.is_forbidden() {
            format!("Cloudflare refused the token: {e}")
        } else {
            e.to_string()
        }
    })?;
    if accounts.is_empty() {
        return Err("this token does not reach any Cloudflare account".into());
    }
    secrets::set_token(token.trim())?;
    log_info!("selfhost", "Connected to Cloudflare ({} account(s))", accounts.len());
    Ok(accounts)
}

/// Há um token guardado neste PC?
#[tauri::command]
pub fn selfhost_has_token() -> Result<bool, String> {
    Ok(secrets::token()?.is_some())
}

#[tauri::command]
pub async fn selfhost_accounts() -> Result<Vec<Account>, String> {
    saved_client()?.accounts().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn selfhost_forget_token() -> Result<(), String> {
    log_info!("selfhost", "Forgot the Cloudflare token on this PC");
    secrets::forget_token()
}

/// Monta um servidor novo: subdomínio, chave do TURN, Worker, endereço e senha.
#[tauri::command]
pub async fn selfhost_create(app: AppHandle, account_id: String, name: String, password: String) -> Result<OwnedServer, String> {
    let (name, password) = check_room_fields(&name, &password)?;
    let cf = saved_client()?;
    let script = format!("{SCRIPT_PREFIX}{}", random_hex(3));
    log_info!("selfhost", "Creating the server \"{name}\" ({script})");

    progress(&app, "subdomain");
    let subdomain = subdomain_or_create(&cf, &account_id).await?;
    let url = worker_url(&script, &subdomain);

    progress(&app, "turn");
    let (turn_key, turn_error) = match cf.create_turn_key(&account_id, &script).await {
        Ok(k) if k.secret.is_some() => (Some(k), None),
        Ok(k) => {
            let _ = cf.delete_turn_key(&account_id, &k.uid).await;
            (None, Some("Cloudflare did not return the relay server (TURN) key".to_string()))
        }
        Err(e) => (None, Some(turn_permission_hint(&e))),
    };
    if let Some(err) = &turn_error {
        log_warn!("selfhost", "The server will have no relay server (TURN): {err}");
    }

    progress(&app, "upload");
    let admin_key = random_hex(32);
    let admin_hash = sha256_hex(&admin_key);
    let mut secret_list = vec![("ADMIN_KEY_HASH", admin_hash.as_str())];
    if let Some(k) = &turn_key {
        secret_list.push(("TURN_KEY_ID", k.uid.as_str()));
        secret_list.push(("TURN_KEY_TOKEN", k.secret.as_deref().unwrap_or_default()));
    }
    let turn_uid = turn_key.as_ref().map(|k| k.uid.clone());
    if let Err(e) = cf.upload_worker(&account_id, &script, &metadata(true, &secret_list), WORKER_JS).await {
        undo_turn(&cf, &account_id, turn_uid.as_deref()).await;
        log_error!("selfhost", "Could not upload the server: {e}");
        return Err(if e.is_forbidden() {
            format!("the token has no permission to create Workers: {e}")
        } else {
            format!("could not upload the server: {e}")
        });
    }
    // a partir daqui a chave tem que estar guardada, ou ninguém administra o servidor
    if let Err(e) = secrets::set_admin_key(&script, &admin_key) {
        let _ = cf.delete_script(&account_id, &script).await;
        undo_turn(&cf, &account_id, turn_uid.as_deref()).await;
        return Err(e);
    }
    if let Err(e) = cf.enable_workers_dev(&account_id, &script).await {
        let _ = cf.delete_script(&account_id, &script).await;
        undo_turn(&cf, &account_id, turn_uid.as_deref()).await;
        let _ = secrets::forget_admin_key(&script);
        log_error!("selfhost", "Could not turn on the server address: {e}");
        return Err(format!("could not turn on the server address: {e}"));
    }

    progress(&app, "online");
    let online = wait_online(&url, Duration::from_secs(180)).await;
    progress(&app, "configure");
    let configured = online && configure_room(&url, &admin_key, &name, &password).await.is_ok();
    if configured {
        log_info!("selfhost", "The server \"{name}\" is online at {url}");
    } else {
        log_warn!("selfhost", "The server \"{name}\" was created at {url}, but it does not answer yet; the room password is not set");
    }
    progress(&app, "done");
    Ok(OwnedServer {
        script_name: script,
        account_id,
        name,
        url,
        turn_key_id: turn_key.map(|k| k.uid),
        turn_error,
        configured,
    })
}

/// Tenta de novo definir nome e senha (quando o servidor demorou a responder).
#[tauri::command]
pub async fn selfhost_configure(script_name: String, url: String, name: String, password: String) -> Result<(), String> {
    let (name, password) = check_room_fields(&name, &password)?;
    let key = secrets::admin_key(&script_name)?.ok_or("this PC has no administrator key for this server")?;
    if !wait_online(&url, Duration::from_secs(30)).await {
        return Err("the server does not answer yet".into());
    }
    configure_room(&url, &key, &name, &password).await
}

/// Sobe o código desta versão do app, mantendo segredos e salas.
#[tauri::command]
pub async fn selfhost_update(account_id: String, script_name: String) -> Result<(), String> {
    let cf = saved_client()?;
    cf.upload_worker(&account_id, &script_name, &metadata(false, &[]), WORKER_JS)
        .await
        .map_err(|e| format!("could not update the server: {e}"))?;
    log_info!("selfhost", "Updated the server {script_name}");
    Ok(())
}

/// Apaga o servidor (a sala e os dados dela) e a chave do TURN.
#[tauri::command]
pub async fn selfhost_delete(account_id: String, script_name: String, turn_key_id: Option<String>) -> Result<(), String> {
    let cf = saved_client()?;
    cf.delete_script(&account_id, &script_name).await.map_err(|e| format!("could not delete the server: {e}"))?;
    if let Some(uid) = turn_key_id {
        if let Err(e) = cf.delete_turn_key(&account_id, &uid).await {
            log_warn!("selfhost", "Deleted the server, but not its relay server (TURN) key: {e}");
        }
    }
    let _ = secrets::forget_admin_key(&script_name);
    log_info!("selfhost", "Deleted the server {script_name}");
    Ok(())
}

/// Põe o TURN num servidor que ficou sem (o token não tinha a permissão do
/// Realtime na hora de montar): cria a chave e grava os dois segredos.
#[tauri::command]
pub async fn selfhost_add_turn(account_id: String, script_name: String) -> Result<String, String> {
    let cf = saved_client()?;
    let key = cf.create_turn_key(&account_id, &script_name).await.map_err(|e| turn_permission_hint(&e))?;
    let Some(secret) = key.secret.as_deref() else {
        let _ = cf.delete_turn_key(&account_id, &key.uid).await;
        return Err("Cloudflare did not return the relay server (TURN) key".into());
    };
    let saved = async {
        cf.set_secret(&account_id, &script_name, "TURN_KEY_ID", &key.uid).await?;
        cf.set_secret(&account_id, &script_name, "TURN_KEY_TOKEN", secret).await
    };
    if let Err(e) = saved.await {
        let _ = cf.delete_turn_key(&account_id, &key.uid).await;
        return Err(format!("could not give the relay server (TURN) key to the server: {e}"));
    }
    log_info!("selfhost", "Added a relay server (TURN) to the server {script_name}");
    Ok(key.uid)
}

/// A chave de administrador deste PC para um servidor: o app entra na sala
/// como dono com ela (fica no mesmo PC; nunca vai para outra pessoa).
#[tauri::command]
pub fn selfhost_admin_key(script_name: String) -> Result<Option<String>, String> {
    secrets::admin_key(&script_name)
}

/// A senha guardada de um servidor de outra pessoa em que já entrei (`None`
/// se não houver): o app só pede a senha na primeira vez.
#[tauri::command]
pub fn server_password(url: String) -> Result<Option<String>, String> {
    secrets::server_password(&url)
}

/// Guarda a senha que funcionou ao entrar num servidor de outra pessoa.
#[tauri::command]
pub fn server_password_save(url: String, password: String) -> Result<(), String> {
    secrets::set_server_password(&url, &password)
}

/// Esquece a senha de um servidor (tirado da lista, ou a senha mudou).
#[tauri::command]
pub fn server_password_forget(url: String) -> Result<(), String> {
    secrets::forget_server_password(&url)
}

/// Servidores do AbreTela que já existem na conta (montados em outro PC).
#[tauri::command]
pub async fn selfhost_find(account_id: String) -> Result<Vec<FoundServer>, String> {
    let cf = saved_client()?;
    let scripts = cf.scripts(&account_id).await.map_err(|e| e.to_string())?;
    let ours: Vec<String> = scripts.into_iter().filter(|s| s.starts_with(SCRIPT_PREFIX)).collect();
    if ours.is_empty() {
        return Ok(vec![]);
    }
    let subdomain = cf.subdomain(&account_id).await.map_err(|e| e.to_string())?.unwrap_or_default();
    Ok(ours.into_iter().map(|s| FoundServer { url: worker_url(&s, &subdomain), script_name: s }).collect())
}

/// Passa a administrar daqui um servidor que já existe: troca a chave de
/// administrador (a do outro PC para de valer).
#[tauri::command]
pub async fn selfhost_claim(account_id: String, script_name: String) -> Result<OwnedServer, String> {
    let cf = saved_client()?;
    let subdomain = cf.subdomain(&account_id).await.map_err(|e| e.to_string())?.ok_or("the account has no workers.dev address")?;
    let url = worker_url(&script_name, &subdomain);
    let admin_key = random_hex(32);
    cf.set_secret(&account_id, &script_name, "ADMIN_KEY_HASH", &sha256_hex(&admin_key))
        .await
        .map_err(|e| format!("could not take over the server: {e}"))?;
    secrets::set_admin_key(&script_name, &admin_key)?;
    let info: Value = match plain_client(15).get(format!("{url}/api/info")).send().await {
        Ok(r) => r.json().await.unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    let turn_key_id = cf
        .turn_keys(&account_id)
        .await
        .ok()
        .and_then(|keys| keys.into_iter().find(|k| k.name.as_deref() == Some(script_name.as_str())).map(|k| k.uid));
    log_info!("selfhost", "This PC now manages the server {script_name}");
    Ok(OwnedServer {
        name: info["name"].as_str().unwrap_or("AbreTela").to_string(),
        configured: info["configured"].as_bool().unwrap_or(false),
        turn_error: None,
        turn_key_id,
        script_name,
        account_id,
        url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_upload_creates_the_room_class_and_secrets() {
        let m = metadata(true, &[("ADMIN_KEY_HASH", "abc")]);
        assert_eq!(m["main_module"], "worker.js");
        assert_eq!(m["bindings"][0], json!({ "type": "durable_object_namespace", "name": "ROOM", "class_name": "Room" }));
        assert_eq!(m["bindings"][1], json!({ "type": "secret_text", "name": "ADMIN_KEY_HASH", "text": "abc" }));
        assert_eq!(m["migrations"], json!({ "new_tag": "v1", "steps": [{ "new_sqlite_classes": ["Room"] }] }));
        assert!(m.get("keep_bindings").is_none());
    }

    #[test]
    fn updates_keep_the_secrets_and_skip_the_migration() {
        let m = metadata(false, &[]);
        assert!(m.get("migrations").is_none());
        assert_eq!(m["keep_bindings"], json!(["secret_text", "secret_key"]));
        assert_eq!(m["bindings"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn the_worker_is_embedded_and_exports_the_room() {
        assert!(WORKER_JS.contains("Room"));
        assert!(WORKER_JS.contains("/api/admin/"));
    }

    #[test]
    fn admin_key_hash_matches_the_worker() {
        // o Worker compara com SHA-256 em hexadecimal (server/src/crypto.ts)
        assert_eq!(sha256_hex("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(random_hex(32).len(), 64);
        assert_ne!(random_hex(8), random_hex(8));
    }

    #[test]
    fn room_fields_are_checked() {
        assert!(check_room_fields(" Sala ", " 1234 ").is_ok());
        assert!(check_room_fields("", "1234").is_err());
        assert!(check_room_fields("Sala", "  ").is_err());
        assert!(check_room_fields(&"x".repeat(41), "1234").is_err());
    }
}
