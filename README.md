# AbreTela

App de desktop (Windows) para transmitir telas, jogos e aplicativos — com áudio — para os amigos, no estilo do Discord, mas sem chat de voz.

## Como funciona

- **Criar sala**: o app sobe um servidor de sinalização local e abre um túnel do Cloudflare (`trycloudflare.com`) com o `cloudflared` embutido. Gera um convite `código#senha` para mandar aos amigos.
- **Entrar**: cole o convite, ou o código e a senha.
- **Transmitir**: seletor próprio com as abas *Aplicativos* e *Telas*. A captura é nativa (Windows Graphics Capture) e o áudio é capturado por processo (WASAPI process loopback): só o som do app escolhido, sem eco.
- **Assistir**: vídeo e áudio vão direto entre os PCs (WebRTC P2P). Dá para assistir várias transmissões em grade ou focar em uma.
- Quem cria a sala define a qualidade mínima e máxima (360p–1080p, 15–60 fps).

## Stack

- [Tauri 2](https://tauri.app) + Rust (axum, windows-capture, wasapi)
- Svelte 5 + TypeScript + Vite

## Desenvolvimento

Requisitos: Node.js, Rust (toolchain MSVC) e Visual Studio Build Tools.

```sh
npm install
npm run fetch-cloudflared   # baixa o cloudflared para src-tauri/binaries/
npm run tauri dev           # modo desenvolvimento
npm run tauri build         # gera o instalador em src-tauri/target/release/bundle/nsis/
```

## Estrutura

- `src-tauri/src/server.rs`: servidor de sinalização da sala
- `src-tauri/src/tunnel.rs`: inicia o cloudflared e descobre a URL pública
- `src-tauri/src/capture.rs`: captura nativa de vídeo/áudio e WebSocket local dos frames
- `src/lib/room.ts`: cliente da sala e conexões WebRTC
- `src/lib/capture.ts`: transforma os frames nativos em MediaStreamTracks
