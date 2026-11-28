//! Segredos do servidor próprio no Gerenciador de Credenciais do Windows (no
//! Mac, nas Chaves; no Linux, no Secret Service): o token da Cloudflare, a
//! chave de administrador de cada servidor e a senha dos servidores de outras
//! pessoas em que já entrei. Nada disso vai para o `localStorage` do WebView.

const SERVICE: &str = "AbreTela";
const TOKEN: &str = "cloudflare-token";

fn entry(user: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, user).map_err(|e| format!("could not open the credential store: {e}"))
}

fn admin_user(script: &str) -> String {
    format!("server-admin:{script}")
}

/// O mesmo servidor escrito de jeitos diferentes (barra no fim, maiúsculas)
/// guarda a senha num lugar só, como o `sameServer` do frontend.
fn password_user(url: &str) -> String {
    format!("server-password:{}", url.trim().trim_end_matches('/').to_lowercase())
}

fn get(user: &str) -> Result<Option<String>, String> {
    match entry(user)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("could not read from the credential store: {e}")),
    }
}

fn set(user: &str, value: &str) -> Result<(), String> {
    entry(user)?.set_password(value).map_err(|e| format!("could not save to the credential store: {e}"))
}

fn delete(user: &str) -> Result<(), String> {
    match entry(user)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("could not delete from the credential store: {e}")),
    }
}

pub fn token() -> Result<Option<String>, String> {
    get(TOKEN)
}

pub fn set_token(token: &str) -> Result<(), String> {
    set(TOKEN, token)
}

pub fn forget_token() -> Result<(), String> {
    delete(TOKEN)
}

pub fn admin_key(script: &str) -> Result<Option<String>, String> {
    get(&admin_user(script))
}

pub fn set_admin_key(script: &str, key: &str) -> Result<(), String> {
    set(&admin_user(script), key)
}

pub fn forget_admin_key(script: &str) -> Result<(), String> {
    delete(&admin_user(script))
}

pub fn server_password(url: &str) -> Result<Option<String>, String> {
    get(&password_user(url))
}

pub fn set_server_password(url: &str, password: &str) -> Result<(), String> {
    set(&password_user(url), password)
}

pub fn forget_server_password(url: &str) -> Result<(), String> {
    delete(&password_user(url))
}

#[cfg(test)]
mod tests {
    #[test]
    fn one_password_per_server_however_it_is_written() {
        assert_eq!(super::password_user("https://A.workers.dev/"), super::password_user("https://a.workers.dev"));
        assert_ne!(super::password_user("https://a.workers.dev"), super::password_user("https://b.workers.dev"));
    }

    /// Manual (mexe no Gerenciador de Credenciais de verdade): grava, lê por
    /// outra entrada e apaga. Confere que não caiu no armazenamento falso do
    /// keyring, que só guarda na memória.
    #[test]
    #[ignore]
    fn credential_store_keeps_values_between_entries() {
        let user = "test-roundtrip";
        super::set(user, "valor-de-teste").unwrap();
        assert_eq!(super::get(user).unwrap().as_deref(), Some("valor-de-teste"));
        super::delete(user).unwrap();
        assert_eq!(super::get(user).unwrap(), None);
    }
}
