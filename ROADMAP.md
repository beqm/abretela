# Próximos passos

Planos combinados para depois da 0.3.0 (novembro de 2026). Cada fase deve sair
sem mudar nada que o usuário vê e ser testada numa sala real (2+ PCs) antes da
próxima: um bug de conexão no meio de muitos arquivos mudados é difícil de achar.

## Onde o projeto está

- Arquitetura de plugins feita (seção 1, fases 1 a 5, branch
  `refactor/fase-1-pastas`). Cada provedor está atrás de um contrato:
  - Túnel: `src-tauri/src/tunnel/` (`Tunnel` + `cloudflare.rs`).
  - Captura: `src-tauri/src/capture/contracts.rs` com `VideoSource`,
    `AudioSource`, `FrameConverter` e `FrameSink`; o que é do Windows fica
    em `capture/win/` (WGC, D3D11, WASAPI), a conversão no processador em
    `convert.rs` e o WebSocket local em `sink.rs`.
  - Transporte de mídia: `src/lib/transport/types.ts` (`MediaTransport`) e a
    malha atual em `mesh.ts`.
  - Formato dos pacotes da captura: `capture/packet.rs` e
    `src/lib/capture/packet.ts`, conferidos pelos mesmos exemplos em
    `src/lib/capture/packet-fixture.json`.
- Frontend em `app/`, `features/` (home, room, settings) e `ui/`; CSS global
  só em `styles/` (tokens, base e controles), o resto no `<style>` de cada
  componente.
- Testes: Vitest (convite, qualidade, idiomas, pacotes, SDP, transporte e o
  `RoomClient` inteiro com WebRTC e servidor falsos) e `cargo test` (servidor
  da sala, conversão de cor, pacotes). CI em todo push (`.github/workflows/ci.yml`).
  Testes manuais com `#[ignore]`: captura (`capture_smoke`) e túnel (`e2e_host`).
  O `capture_smoke` roda sem otimização: no processador cada frame leva ~0,5 s
  ali, o que é normal (na versão final são poucos ms).
- A sala falsa (`?mock` e `?mock=empty` com `npm run dev`) permite mexer na
  interface sem Tauri, mas não usa o `RoomClient` de verdade.
- Bugs achados e corrigidos durante a refatoração: senha errada às vezes virava
  "não foi possível conectar" depois de 45s (RST do Windows no servidor da
  sala) e o app travava ao trocar para uma tela com outras faixas (ex.: sem
  áudio) com alguém assistindo.

---

## 2. Envio nativo (WebRTC no Rust para quem transmite)

**Decidido:** quem transmite passa a capturar, codificar e enviar tudo no Rust;
quem assiste continua usando o WebRTC do WebView (Windows e Mac). É o mesmo
caminho que destrava Linux e Mac (seção 3).

Por quê:

- Acaba com a ponte de frames Rust → WebView por WebSocket local (~180 MB/s em
  1080p60), suspeita principal do caso do Eliaga (abaixo).
- O frame vai da placa de vídeo direto para o codificador (NVENC/AMF/QuickSync
  via Media Foundation no Windows), sem passar pela memória do processador.
- Permite **codificar uma vez e mandar para todos** (hoje é uma codificação por
  espectador).
- Abre espaço para rede mais esperta: UPnP/PCP, tentativa de adivinhar portas,
  repasse próprio.

Como fazer sem quebrar ninguém:

- WebRTC é padrão: um envio nativo conversa com quem assiste pelo WebView
  igual ao de hoje. As mensagens da sala (offer/answer/ice/view/thumb) não
  mudam, então continua compatível com versões antigas.
- Vira uma implementação de `MediaTransport` (fase 3). Entra primeiro como
  opção **experimental**, com o envio pelo WebView como reserva; só vira o
  padrão depois de testado em salas reais.
- **Biblioteca escolhida: `str0m`** (novembro de 2026). Aceita frames já
  codificados (codificar uma vez, pela placa, e mandar para todos), compila
  nos três sistemas sem C++ e usa a criptografia do sistema. A `libwebrtc`
  quer codificar ela mesma e os binários prontos não usam a placa no Windows;
  o `webrtc-rs` está no meio de uma reescrita. Comparação, números e o que o
  protótipo ensinou (limitar quadros-chave, espectador lento puxando os
  outros) em `prototypes/native-send/README.md`.
- Cuidados: perfil de H.264 que todo mundo decodifique; controle de banda tão
  bom quanto o do Chromium.

Partes, cada uma saindo e testada antes da próxima:

- [x] **3a. Codificador H.264 pela placa** (`src-tauri/src/encode/`): contrato
  `VideoEncoder` e Media Foundation (placa de vídeo assíncrona com texturas
  D3D11; o codificador do Windows no processador como reserva). Sai Constrained
  Baseline em Annex B, quadro-chave quando pedido, bitrate mudável, sem segurar
  frames. Medido numa RX 7800 XT: tela real 1080p60 a ~6,4 ms por frame (95%
  abaixo de 7,6 ms), decodifica limpo no ffmpeg. Testes manuais:
  `encode_hardware`, `encode_software` e `capture_encode_smoke` (`--ignored`).
  Ainda sem uso no app. Falta testar em placas NVIDIA e Intel.
- [x] **3b. Transporte nativo** (`src-tauri/src/send/` e
  `src/lib/transport/native.ts`), só vídeo. A captura alimenta a prévia local
  (WebSocket, como antes) e o envio nativo; uma thread codifica o frame mais
  novo uma vez e manda a todos os conectados. Cada espectador tem um `Rtc` do
  str0m, um socket UDP por placa de rede (rota "rede local") e STUN próprio
  (`send/stun.rs`, rota "public address"); as rotas vão e vêm aos poucos pela
  sala (trickle) e as rotas mDNS do navegador são puladas (viram "descobertas"
  na checagem). Já entram: quadro-chave no máximo 1/s, bitrate pela menor
  estimativa, tela parada repete o último frame 1x/s (o WGC só entrega frame
  quando a tela muda; sem isso quem entra vê preto).
  - Ligar: preferência escondida `nativeSend` (no `localStorage`,
    `abretela.prefs` → `"nativeSend": true`), sem opção na tela ainda.
  - Testado: `native_send_browser` (`--ignored`, página só em 127.0.0.1)
    com dois espectadores Chromium: conecta em ~0,1 s, H.264 1080p
    decodificado, 0% de perda por 2,5 min, fecha limpo. Testes do
    `NativeTransport` com o Rust simulado e um espectador da malha.
  - Com a tela em uso (navegador com YouTube): imagem 1080p certa, 0% de
    perda, nenhum pedido de quadro-chave, ~4–6 Mbps, mas ~40–54 fps.
  - **fps corrigido (codificação em fila):** o `encode` esperava cada frame
    sair antes de aceitar o próximo. Agora só entrega e volta; uma thread lê
    os eventos da placa e os frames saem por `OnEncoded` assim que ficam
    prontos. O log mostra por etapa (cópia para a placa, espera para entrar,
    tempo dentro do codificador). Medido: copiar é 0,2 ms; o codificador da
    AMD leva ~6 ms por frame 1080p, mesmo com ruído (`ENCODE_NOISE=1`), e de
    ponta a ponta com frames sintéticos (`NATIVE_SYNTH=1`) o navegador recebe
    **60 fps cravados**, 8 Mbps, 0% de perda.
  - Na tela real com um vídeo tocando, o mesmo codificador chegou a ~16 ms
    por frame: a AMD só aceita o próximo frame depois de terminar o anterior,
    e a parte da placa que codifica é a mesma que decodifica o vídeo que está
    tocando. Mudar "qualidade x velocidade" não ajudou. O envio pelo WebView
    sofre o mesmo (e codifica uma vez por espectador). Ver de novo na 3e, na
    comparação lado a lado.
  - Falta testar: sala real entre PCs/redes diferentes e com o app nos dois
    lados.
  - Notado: com a tela parada o codificador da AMD ainda gasta ~16 KB por
    frame repetido (enche até o bitrate); ver na 3d.
  - A prévia local ainda passa pela ponte WebSocket: para medir o ganho de
    verdade (3e) ela precisa ficar leve (menos fps/resolução) no modo nativo.
- [ ] **3c. Áudio Opus** no envio nativo.
- [ ] **3d. Políticas de banda**: limite de quadros-chave, espectador lento,
  qualidade por espectador.
- [ ] **3e. Opção experimental** nas configurações, comparação lado a lado com
  o envio pelo WebView e teste em sala real.
- Depois: entregar a textura da captura direto ao codificador (hoje o NV12
  passa pela memória do processador uma vez).

O que **não** muda: a borda amarela no Windows 10 vem da captura (Windows
Graphics Capture), não do WebRTC. Sem borda no Windows 10 só capturando o
monitor inteiro (DXGI Desktop Duplication); injetar no jogo como o OBS está
descartado (antitrapaça).

## 3. Linux e Mac

Depende da fase 4 e do envio nativo (seção 2). O que hoje é só do Windows:

| Parte | Windows (hoje) | Mac | Linux |
|---|---|---|---|
| Captura de tela/janela | Windows Graphics Capture | ScreenCaptureKit (macOS 12.3+) | PipeWire via xdg-desktop-portal (Wayland) / X11 |
| Áudio por app | WASAPI process loopback | ScreenCaptureKit (áudio, macOS 13+) | PipeWire (por app nem sempre dá) |
| Conversão na placa | D3D11 (`gpu_convert.rs`) | Metal/VideoToolbox ou só CPU | VA-API ou só CPU |
| Túnel | `cloudflared.exe` | `cloudflared` darwin (arm64/x64) | `cloudflared` linux |
| Instalador | NSIS | `.dmg` | AppImage / `.deb` |

**Bloqueio principal: o navegador embutido muda em cada sistema.**

- Windows usa o WebView2 (Chromium). Mac usa WKWebView (Safari) e Linux usa
  WebKitGTK.
- O app depende de `MediaStreamTrackGenerator` para transformar os frames
  nativos em vídeo para o WebRTC, e isso só existe no Chromium.
- No Linux, o WebKitGTK costuma vir sem WebRTC habilitado.
- **Decisão:** envio nativo em todos os sistemas (seção 2). Para assistir:

  | | Transmitir | Assistir |
  |---|---|---|
  | Windows | nativo | WebView (já funciona) |
  | Mac | nativo (obrigatório) | WebView (Safari tem WebRTC para receber) |
  | Linux | nativo (obrigatório) | provavelmente nativo: decodificar no Rust e mostrar o vídeo (a parte mais trabalhosa; pode vir depois) |

Outros pontos:

- Mac: sem assinatura e notarização da Apple (conta paga de desenvolvedor), o
  Gatekeeper bloqueia o app. Permissão de gravação de tela pedida pelo sistema.
- Linux: no Wayland a escolha da tela é feita pelo diálogo do sistema (portal),
  não pelo seletor do app.
- `fetch-cloudflared` e o `release.yml` passam a baixar e compilar por sistema
  (matriz no GitHub Actions).
- Bandeja, iniciar com o sistema, deep link e instância única: os plugins do
  Tauri já suportam os três sistemas, mas precisam de teste.

---

## 4. Conectividade sem servidor pago

Hoje só há conexão direta (STUN). Quando os dois lados estão em redes fechadas
(CGNAT/NAT simétrico), não existe caminho direto e alguém precisa repassar o
vídeo. Plano em camadas, todas automáticas e sem custo para o dono do app:

1. **Direta** (IPv4/IPv6), como hoje.
2. **Repasse por outro participante** que alcance os dois lados (usa WebRTC
   normal; custa upload de quem repassa). Opção "permitir repassar telas",
   ligada por padrão.
3. **Repasse pelo túnel do anfitrião** (vídeo codificado sobre o WebSocket da
   sala) como último recurso: funciona em qualquer rede, mas com mais atraso,
   gasta upload do anfitrião e não é o uso pensado pela Cloudflare para o túnel
   gratuito.
4. **TURN opcional** (plugin), para quem quiser configurar a própria chave.

Cada camada vira uma implementação de `MediaTransport` (fase 3).

---

## Pendências do caso do Eliaga

Ninguém consegue assistir a tela dele; ele assiste todo mundo. Outra pessoa na
mesma internet funciona, e o firewall desligado não muda nada: não é rede nem
firewall.

- Ele passou para o Windows 11 e continua sem funcionar. Cai a suspeita de
  sobrecarga por falta de limite de fps no Windows 10; as correções só para o
  Windows 10 (limitar fps, capturar no tamanho do espectador) ficam de lado.
- Próximo passo: diagnóstico detalhado no log quando a conexão falha (etapa
  ICE ou DTLS, pares de rota testados e respostas, portas públicas) e pegar o
  log dos dois lados numa tentativa. Os logs de rede agora ficam todos em
  `src/lib/transport/diagnostics.ts`.
- Se for algo na ponte de frames dele, o envio nativo (seção 2) elimina a ponte.

---

## Outras ideias levantadas (sem ordem)

- **TURN** para quem não conecta direto: ver a seção 4.
- Mostrar **quem está te assistindo** enquanto você transmite.
- **Notificação do Windows** quando alguém abre a tela, com clique para assistir.
- **Atalhos globais** (ex.: abrir/fechar sua tela sem sair do jogo).
- Chat de texto ou reações leves na sala.
- Janela flutuante própria (título "Nome - AbreTela") no lugar do
  Picture-in-Picture do navegador, que mostra `tauri.localhost` no topo.
- Indicador de fps e atraso reais em cada tela.
- Mensagens de erro mais claras (túnel, captura).
- Anfitrião poder remover alguém da sala.
- SFU (Cloudflare Realtime ou LiveKit) só se as salas crescerem além de ~10
  pessoas transmitindo ao mesmo tempo.
