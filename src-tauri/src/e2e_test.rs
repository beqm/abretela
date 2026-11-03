//! Teste manual: sobe servidor + túnel e mantém aberto por 90s.
//! `cargo test e2e -- --ignored --nocapture`
use crate::{server, tunnel};

#[tokio::test]
#[ignore]
async fn e2e_host() {
    let settings = server::RoomSettings { min_height: 360, max_height: 1080, min_fps: 15, max_fps: 60 };
    let page = crate::invite_page::InvitePage { host_name: "Teste".into(), lang: "pt".into() };
    let srv = server::start(settings, "1234".into(), page).await.unwrap();
    let tunnel = tunnel::open(srv.port).await.unwrap();
    println!("TUNNEL_URL={}/#1234", tunnel.public_url());
    tokio::time::sleep(std::time::Duration::from_secs(90)).await;
}
