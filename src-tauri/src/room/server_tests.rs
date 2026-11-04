//! Teste de integração do servidor da sala: sobe o servidor de verdade numa
//! porta local e conversa com ele como o app faz (WebSocket + HTTP).

use super::invite_page::InvitePage;
use super::server::{self, RoomSettings, RunningServer};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const PASSWORD: &str = "123456";
const SETTINGS: RoomSettings = RoomSettings { min_height: 480, max_height: 1080, min_fps: 30, max_fps: 60 };

async fn start_room() -> RunningServer {
    let page = InvitePage { host_name: "Ana".into(), lang: "pt".into() };
    server::start(SETTINGS, PASSWORD.into(), page).await.unwrap()
}

async fn connect(srv: &RunningServer) -> Ws {
    let (ws, _) = connect_async(format!("ws://127.0.0.1:{}/ws", srv.port)).await.unwrap();
    ws
}

async fn send(ws: &mut Ws, value: Value) {
    ws.send(Message::text(value.to_string())).await.unwrap();
}

/// Próxima mensagem de texto (ignora os pings), ou `None` se a conexão fechou.
async fn next(ws: &mut Ws) -> Option<Value> {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .expect("o servidor não respondeu a tempo");
        match msg {
            Some(Ok(Message::Text(t))) => return Some(serde_json::from_str(t.as_str()).unwrap()),
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return None,
            Some(Ok(_)) => continue,
        }
    }
}

async fn recv(ws: &mut Ws) -> Value {
    next(ws).await.expect("a conexão fechou")
}

/// Entra na sala e devolve a conexão, o id recebido e o "welcome" inteiro.
async fn join(srv: &RunningServer, name: &str) -> (Ws, String, Value) {
    let mut ws = connect(srv).await;
    send(&mut ws, json!({ "type": "join", "name": name, "password": PASSWORD })).await;
    let welcome = recv(&mut ws).await;
    assert_eq!(welcome["type"], "welcome");
    let id = welcome["id"].as_str().unwrap().to_string();
    (ws, id, welcome)
}

/// GET simples por HTTP/1.1; devolve a resposta inteira (cabeçalhos + corpo).
async fn http_get(srv: &RunningServer, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", srv.port)).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: sala.example.com\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

#[tokio::test]
async fn wrong_password_gets_an_error_and_is_disconnected() {
    let srv = start_room().await;
    let mut ws = connect(&srv).await;
    send(&mut ws, json!({ "type": "join", "name": "Intruso", "password": "000000" })).await;
    assert_eq!(recv(&mut ws).await, json!({ "type": "error", "reason": "wrong-password" }));
    assert_eq!(next(&mut ws).await, None);

    // ninguém entrou: a página do convite continua mostrando a sala vazia
    assert!(http_get(&srv, "/").await.contains("0 pessoas na sala"));
}

#[tokio::test]
async fn right_password_gets_welcome_with_people_and_settings() {
    let srv = start_room().await;
    // espaços em volta da senha são ignorados
    let mut ws = connect(&srv).await;
    send(&mut ws, json!({ "type": "join", "name": "  Ana  ", "password": " 123456 " })).await;
    let welcome = recv(&mut ws).await;
    assert_eq!(welcome["type"], "welcome");
    assert_eq!(welcome["peers"], json!([]));
    assert_eq!(
        welcome["settings"],
        json!({ "minHeight": 480, "maxHeight": 1080, "minFps": 30, "maxFps": 60 })
    );
    let ana = welcome["id"].as_str().unwrap().to_string();

    let (_bia_ws, bia, welcome) = join(&srv, "Bia").await;
    assert_ne!(ana, bia);
    assert_eq!(welcome["peers"], json!([{ "id": ana, "name": "Ana", "sharing": false, "label": null }]));

    // quem já estava fica sabendo
    assert_eq!(
        recv(&mut ws).await,
        json!({ "type": "peer-joined", "peer": { "id": bia, "name": "Bia", "sharing": false, "label": null } })
    );
}

#[tokio::test]
async fn names_are_trimmed_and_get_a_default() {
    let srv = start_room().await;
    let (_ws, nameless, _) = join(&srv, "   ").await;
    let (_ws2, _, welcome) = join(&srv, &"x".repeat(50)).await;
    assert_eq!(welcome["peers"][0]["name"], format!("Convidado {nameless}"));

    let (_ws3, _, welcome) = join(&srv, "Caio").await;
    let names: Vec<&str> = welcome["peers"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"x".repeat(32).as_str()), "{names:?}");
}

#[tokio::test]
async fn signals_are_relayed_only_to_the_target() {
    let srv = start_room().await;
    let (mut ana_ws, ana, _) = join(&srv, "Ana").await;
    let (mut bia_ws, bia, _) = join(&srv, "Bia").await;
    let (mut caio_ws, _, _) = join(&srv, "Caio").await;
    // avisos de entrada que chegaram antes
    recv(&mut ana_ws).await;
    recv(&mut ana_ws).await;
    recv(&mut bia_ws).await;

    let data = json!({ "kind": "offer", "sdp": "v=0..." });
    send(&mut ana_ws, json!({ "type": "signal", "to": bia, "data": data })).await;
    assert_eq!(recv(&mut bia_ws).await, json!({ "type": "signal", "from": ana, "data": data }));

    // para alguém que não existe: some sem erro
    send(&mut ana_ws, json!({ "type": "signal", "to": "999", "data": {} })).await;

    // Caio não recebeu nenhum dos dois: a próxima coisa que chega é a saída da Bia
    drop(bia_ws);
    assert_eq!(recv(&mut caio_ws).await, json!({ "type": "peer-left", "id": bia }));
}

#[tokio::test]
async fn messages_before_join_are_ignored() {
    let srv = start_room().await;
    let (mut ana_ws, ana, _) = join(&srv, "Ana").await;
    let mut ws = connect(&srv).await;
    send(&mut ws, json!({ "type": "signal", "to": ana, "data": { "x": 1 } })).await;
    send(&mut ws, json!({ "type": "sharing", "sharing": true, "label": "720p" })).await;
    send(&mut ws, json!("lixo")).await;
    // depois de tudo isso ainda dá para entrar normalmente
    send(&mut ws, json!({ "type": "join", "name": "Bia", "password": PASSWORD })).await;
    assert_eq!(recv(&mut ws).await["type"], "welcome");
    // e a Ana só ficou sabendo da entrada
    assert_eq!(recv(&mut ana_ws).await["type"], "peer-joined");
}

#[tokio::test]
async fn sharing_is_broadcast_and_shows_on_the_invite_page() {
    let srv = start_room().await;
    let (mut ana_ws, ana, _) = join(&srv, "Ana").await;
    let (mut bia_ws, _, _) = join(&srv, "Bia").await;
    recv(&mut ana_ws).await; // Bia entrou

    send(&mut ana_ws, json!({ "type": "sharing", "sharing": true, "label": "720p 60fps · Jogo" })).await;
    let update = json!({ "type": "peer-updated", "id": ana, "sharing": true, "label": "720p 60fps · Jogo" });
    assert_eq!(recv(&mut bia_ws).await, update);
    assert_eq!(recv(&mut ana_ws).await, update);

    let page = http_get(&srv, "/").await;
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("2 pessoas na sala · 1 ao vivo"));
    assert!(page.contains("Sala de Ana no AbreTela"));
    assert!(page.contains("https://sala.example.com/logo.png"));

    // parou de transmitir: o rótulo some
    send(&mut ana_ws, json!({ "type": "sharing", "sharing": false, "label": "sobrou" })).await;
    assert_eq!(recv(&mut bia_ws).await, json!({ "type": "peer-updated", "id": ana, "sharing": false, "label": null }));
}

#[tokio::test]
async fn leaving_is_announced_to_the_rest() {
    let srv = start_room().await;
    let (mut ana_ws, _, _) = join(&srv, "Ana").await;
    let (mut bia_ws, bia, _) = join(&srv, "Bia").await;
    recv(&mut ana_ws).await; // Bia entrou

    bia_ws.close(None).await.unwrap();
    assert_eq!(recv(&mut ana_ws).await, json!({ "type": "peer-left", "id": bia }));
    assert!(http_get(&srv, "/").await.contains("1 pessoa na sala"));
}

#[tokio::test]
async fn closing_the_room_disconnects_everyone() {
    let srv = start_room().await;
    let (mut ana_ws, _, _) = join(&srv, "Ana").await;
    srv.stop.send(true).unwrap();
    assert_eq!(next(&mut ana_ws).await, None);
}

#[tokio::test]
async fn serves_health_and_logo() {
    let srv = start_room().await;
    let health = http_get(&srv, "/health").await;
    assert!(health.starts_with("HTTP/1.1 200") && health.ends_with("ok"), "{health}");
    let logo = http_get(&srv, "/logo.png").await;
    assert!(logo.to_lowercase().contains("content-type: image/png"), "{logo}");
}
