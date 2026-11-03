//! Túnel que publica o servidor local da sala na internet. O resto do app só
//! conhece este contrato: abrir para uma porta local, receber a URL https
//! pública e fechar. Trocar de provedor = escrever outra implementação e
//! escolher ela em `open`.

mod cloudflare;

pub trait Tunnel: Send {
    /// URL https pública que leva ao servidor local, sem barra no fim.
    fn public_url(&self) -> &str;
    /// Encerra o túnel (sem esperar ele terminar).
    fn close(&mut self);
}

/// Abre um túnel com o provedor padrão para `http://127.0.0.1:{port}`.
pub async fn open(port: u16) -> Result<Box<dyn Tunnel>, String> {
    let tunnel = cloudflare::CloudflareTunnel::open(port).await?;
    Ok(Box::new(tunnel))
}
