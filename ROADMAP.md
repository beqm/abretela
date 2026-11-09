# Próximos passos

Planos combinados para depois da 0.3.0 (novembro de 2026). Cada fase deve sair
sem mudar nada que o usuário vê e ser testada numa sala real (2+ PCs) antes da
próxima: um bug de conexão no meio de muitos arquivos mudados é difícil de achar.

**Como trabalhar (desde a 0.5.0):** tudo vai para a branch `dev`. Para lançar
uma versão, abre um PR de `dev` para `main`; o dono do projeto revisa e aceita.
Só depois do PR aceito entram o CHANGELOG final, a versão e a tag `v*` (que
publica o app para todo mundo pelo atualizador).

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
  - Ligar: Configurações → Geral → Desempenho → "Envio nativo
    (experimental)" (3e).
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
    frame repetido (enche até o bitrate); resolvido na 3d (repete só a cada
    5 s ou quando alguém pede quadro-chave).
  - A prévia local ainda passa pela ponte WebSocket: para medir o ganho de
    verdade (3e) ela precisa ficar leve (menos fps/resolução) no modo nativo.
- [x] **3c. Áudio Opus** no envio nativo (`send/audio.rs`). O som da captura
  (f32 estéreo 48 kHz, pedaços de 10 ms) vira Opus em pacotes de 20 ms, 128
  kbps, FEC ligado, codificado uma vez para todos. Sai numa segunda faixa no
  mesmo stream do vídeo ("abretela"), então quem assiste recebe uma
  `MediaStream` com as duas, como hoje; o tempo de cada pacote vem do mesmo
  relógio do vídeo, para sincronizar. Sem som na captura, a oferta vai sem
  faixa de áudio.
  - Codificador: `opus-pure` (Rust puro, sem C nem cmake; o `opus`/libopus
    pede cmake, que não está instalado e teria que entrar no CI e no Mac e
    Linux). Comparada com a `opus-rs`: as duas ~0,1 ms por pacote, e o
    resultado de ambas decodifica limpo na libopus (ffmpeg); a `opus-pure` não
    tem dependências e mediu um pouco melhor (SNR 25,3 contra 23,4 dB).
  - Quando o app fica quieto o Windows para de mandar pedaços; na volta o tempo
    pula para "agora" (`RESYNC_AFTER`), em vez de o som ficar atrasado.
  - Testado: `native_send_browser` com um tom gerado (440 Hz, 1,5 s de silêncio
    a cada 6 s; `NATIVE_AUDIO=0` sem som). O navegador recebe Opus estéreo,
    ~128 kbps, 0 pacotes perdidos, áudio e vídeo na mesma stream; gravado no
    navegador, chega 440 Hz no nível certo, e a pausa some e volta limpa. O
    vídeo continua em 60 fps com o áudio junto.
  - Falta: testar com som real de um app numa sala; trocar de tela com e sem
    som com alguém já assistindo (quem já está conectado fica com as faixas que
    combinou até reconectar).
- [x] **3d. Políticas de banda** (`send/layers.rs`, `send/scale.rs`): camadas
  de qualidade, um "simulcast" feito no próprio app. Uma codificação por
  camada em uso (a da transmissão, 720p, 480p, 360p), não por pessoa; cada
  espectador recebe uma:
  - a que pediu: a mensagem "view" agora vai para o Rust
    (`native_viewer_view`), com as mesmas regras do envio pelo WebView
    (`encodingFor`: tabela `videoBitrate`, miniatura até 480p a no máximo
    30 fps, 0 = app escondido, só som);
  - ou uma menor, se a conexão não aguenta: desce com a banda abaixo da
    metade do que a camada pede por 3 s seguidos, sobe depois de 10 s com
    folga. Dentro de uma camada o bitrate segue o mais lento dela, mas nunca
    abaixo do ponto em que ele desceria: um espectador lento não derruba os
    outros.
  - O frame é reduzido no processador (`fast_image_resize`, plano por plano,
    ~1–3 ms). Ao trocar de camada o espectador espera um quadro-chave dela.
  - Controle de banda do str0m (`bwe().set_bitrate`): o mínimo é o bitrate da
    camada (o str0m completa com enchimento, sem passar da estimativa) e,
    abaixo da camada pedida, o da camada de cima, com sondagem a cada 5 s.
    Sem isso a estimativa ficava presa no que a mídia mandava, caía e
    derrubava espectador à toa, e quem desceu nunca subia de volta.
  - Codificador espera no máximo 12 ms para aceitar um frame (antes 100 ms):
    uma camada ocupada não segura as outras. O log de cada camada diz quantos
    frames saíram e quantos foram pulados por o codificador estar ocupado.
  - Tela parada: repete o último frame só a cada 5 s (antes 1 s) ou na hora em
    que alguém pede quadro-chave.
  - Testado com três abas ao mesmo tempo (`?view=480` pede menos; `?cap=3000`
    finge conexão de 3 Mbps): cada uma recebe a sua camada (1080p, 480p,
    720p), 0% de perda; uma que fica "lenta" desce para 720p em ~3 s e volta
    para 1080p ~11 s depois de melhorar, sem mexer nas outras. Neste PC, com as
    três abas decodificando na mesma placa, saem ~40 fps por camada (~20 ms
    por frame); com três abas numa camada só dá o mesmo, então o custo é das
    abas decodificando, não das camadas.
  - Ver no teste real: a camada de 1080p com três abas ficou em ~3,3 Mbps
    (a estimativa de um deles caiu), mesmo em rede local.
- [x] **3e. Opção experimental** (falta só o teste em sala real, abaixo).
  - Chave "Envio nativo (experimental)" em Configurações → Desempenho, nos seis
    idiomas, desligada por padrão. Vale a partir da próxima sala: o modo fica
    preso ao `RoomClient` (`client.nativeSend`), e a captura segue o modo da
    sala, não a opção (mudar no meio da sala descasaria transporte e captura, e
    ninguém veria nada).
  - "Aceleração de hardware" desligada também vale para o envio nativo
    (codifica no processador).
  - Prévia local leve: no modo nativo a sua própria tela vai para o app a no
    máximo 15 fps (`PREVIEW_FPS`), e o `Tee` só copia os frames que a prévia
    vai mostrar (antes copiava 3 MB por frame a 60 fps). A resolução da
    prévia continua a da captura.
  - Medição lado a lado (`logging/perf.rs`): a cada 30 s transmitindo, uma linha
    `[perf]` com o processador do app e dos processos do WebView2 (% do PC) e o
    uso da placa por parte (codificação, 3D, decodificação, do contador "GPU
    Engine" do Windows; na AMD a codificação aparece como "Video Codec"). Diz
    qual método está em uso. Medido: 1080p60 nativo = ~1,3% do processador e
    ~15% da parte de codificação da placa (RX 7800 XT). Junto com as linhas de
    saúde de cada espectador (fps, perda), que os dois métodos já tinham.
  - Não tem: voltar sozinho para o envio pelo WebView se o nativo falhar (o
    codificador do processador já é a reserva dentro do nativo).
  - **Roteiro do teste em sala** (2 PCs, redes diferentes de preferência):
    1. Sem a opção: entrar na sala, transmitir 1080p60 com som de um app
       (vídeo ou jogo), a outra pessoa assiste ~2 min. Repetir com 2 ou 3
       pessoas assistindo, se der.
    2. Ligar a opção, sair e entrar de novo na sala, e repetir igual.
    3. Comparar nos logs dos dois PCs: `[perf]` (processador e placa de quem
       transmite), as linhas de saúde (fps e perda de quem assiste) e se o som
       chega sincronizado. Conferir também trocar de tela no meio.
- Depois: entregar a textura da captura direto ao codificador (hoje o NV12
  passa pela memória do processador uma vez).

O que **não** muda: a borda amarela no Windows 10 vem da captura (Windows
Graphics Capture), não do WebRTC. Sem borda no Windows 10 só capturando o
monitor inteiro (DXGI Desktop Duplication); injetar no jogo como o OBS está
descartado (antitrapaça).

## 5. Assistir pelo navegador

Quem recebe o link e não tem o app (ou está no celular) assiste pelo navegador,
sem instalar nada. Só assistir: transmitir continua sendo pelo app.

**Por que dá:** o servidor da sala já sai pelo túnel com a página do convite
(`/`) e a sinalização (`/ws`), e assistir é WebRTC recebendo, que todo
navegador tem. Quem transmite não muda nada: para ele, um espectador do
navegador é igual a um do app (o envio nativo já foi testado com abas do
Chromium assistindo). A senha já vem no link, depois do `#` (o navegador nunca
a manda ao servidor), então ninguém precisa digitar.

**O que já é web e o que depende do Tauri** (levantado em novembro de 2026):
- Web puro, reaproveitado: `RoomClient` (`lib/room.ts`), `Signaling`,
  `MeshTransport` (o lado de assistir), `Thumbs`, `quality.ts`, `invite.ts`,
  idiomas (`locales/`), `Tile`, `Avatar`, `Icon`.
- Depende do Tauri: `log.ts` (`invoke("log")`), tela cheia do `Tile`
  (`getCurrentWindow().setFullscreen`), `Room.svelte` (`set_live`,
  `close_room`, captura, `SourcePicker`), `App.svelte` (deep link, bandeja),
  `Home`, `SettingsPage`, `update.ts`, `capture/native.ts`,
  `transport/native.ts`. Os imports do `@tauri-apps` carregam no navegador; só
  as chamadas falham, então dá para separar sem reescrever.

**Fases** (cada uma testada antes da próxima, tudo na `dev`):

- [ ] **W1. Núcleo sem Tauri.** `isTauri()` num lugar só; `log.ts` cai no
  console fora do app; tela cheia pelo `document.requestFullscreen` quando não
  é o app. Testes do `RoomClient` rodando como no navegador (sem
  `__TAURI_INTERNALS__`). Nada muda no app.
- [ ] **W2. Tela de assistir na web.** Segunda entrada do Vite (`watch.html` +
  `src/web/`), saindo no mesmo `dist/`:
  - pede só o nome (guardado no `localStorage` do navegador), lê a sala da
    própria página (`wss://<host>/ws`) e a senha do `#`;
  - lista quem está ao vivo, prévia ao passar o mouse, assistir uma ou várias
    telas (`Tile`), tela cheia, janela flutuante do próprio navegador
    (Picture-in-Picture), qualidade pelo tamanho da tela (o "view" que o app já
    manda);
  - som: o navegador só toca som depois de um clique na página; o botão de
    entrar conta como esse clique (o Safari é mais chato: botão "ativar som" se
    precisar);
  - funciona no celular (layout de uma coluna, toque);
  - nos seis idiomas, pelo idioma do navegador;
  - sem configurações, bandeja, atualizações nem transmitir; um aviso "para
    transmitir a sua tela, baixe o app" com o link.
- [ ] **W3. Servidor e convite.**
  - O servidor da sala serve `/watch` e os arquivos do Vite (`/assets/...`)
    direto dos arquivos que o Tauri já embute no executável
    (`app.asset_resolver()`), sem empacotar de novo. No `tauri dev` os
    arquivos vêm do servidor do Vite: repassar para ele, ou só funcionar no
    build.
  - A página do convite ganha "Assistir no navegador" nos seis idiomas. No
    celular e fora do Windows ela não tenta abrir o app (não existe lá) e
    esse botão vira o principal.
  - Cabeçalhos: `Content-Type` certo, cache para `/assets` (os nomes já têm
    hash), nada de cache para `/watch`.
- [ ] **W4. Segurança.**
  - Limite de tentativas de senha por pessoa: atrás do túnel tudo chega de
    127.0.0.1, então contar pelo `CF-Connecting-IP` (ex.: 5 erradas por minuto,
    depois espera).
  - Limite de pessoas na sala e de tamanho das mensagens, se ainda não houver.
  - Conferir que a página não expõe nada além do que o app já expõe (só
    arquivos do frontend, sem segredo).
- [ ] **W5. "No navegador" na lista.** O `join` ganha `client: "web"` (o
  servidor e as versões antigas ignoram campos que não conhecem); quem usa o
  app vê um ícone de navegador ao lado do nome, e o log diz "watching from a
  browser".
- [ ] **W6. Testes.** Unidade (rotas, página do convite, limite de senha);
  ponta a ponta com o navegador de verdade: sala + `/watch` servidos localmente
  e uma aba assistindo uma transmissão (WebView e nativa), como nos testes do
  envio nativo. Chrome, Edge, Firefox (H.264 pelo OpenH264 que ele baixa) e, se
  der, Safari/iPhone e Android.
- [ ] **W7. Lançar** (0.6.0): CHANGELOG, PR `dev` → `main`, e só então a tag.

**Limites (ficam como estão):**
- Só assistir. Transmitir pelo navegador daria (`getDisplayMedia`), mas pior
  que o app e sem som por programa; fica de fora.
- A mesma conectividade do app (só STUN): quem não conecta pelo app também não
  conecta pelo navegador. Isso é a seção 4.
- O link só funciona com o app do anfitrião aberto, como hoje.

---

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
