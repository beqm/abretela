//! Teste manual: sobe servidor + túnel e mantém aberto por 90s.
//! `cargo test e2e -- --ignored --nocapture`
use crate::{server, tunnel};

#[tokio::test]
#[ignore]
async fn e2e_host() {
    let settings = server::RoomSettings { min_height: 360, max_height: 1080, min_fps: 15, max_fps: 60 };
    let srv = server::start(settings, "1234".into()).await.unwrap();
    let (_child, url) = tunnel::start(srv.port).await.unwrap();
    println!("TUNNEL_URL={url}");
    tokio::time::sleep(std::time::Duration::from_secs(90)).await;
}
