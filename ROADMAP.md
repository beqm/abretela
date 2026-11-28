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

**Feito em novembro de 2026**, com estas diferenças do plano abaixo:
- A página (`watch.html`, `src/web/`) é construída num arquivo só, com JS, CSS e
  fontes dentro (`scripts/build-web.mjs` gera `dist-web/watch.html`, ~280 KB; só
  a Nunito latina). O servidor próprio embute esse arquivo no código do Worker e
  serve em `/` (abrir o link custa uma requisição); o servidor do túnel embute no
  app e serve em `/watch`. Não precisou do `asset_resolver` nem da API de
  arquivos estáticos da Cloudflare.
- A página do convite do túnel ganhou "Assistir no navegador" (`/watch` + a senha
  do `#`); no celular ela não tenta abrir o app e esse botão vira o principal.
- `src/lib/env.ts` (`isTauri()`): fora do app o log vai para o console e a tela
  cheia é só a do vídeo.
- O `join` manda `client: "web"`; os dois servidores marcam a pessoa (`web`) e o
  app mostra um ícone de navegador ao lado do nome.
- Segurança no túnel: 5 senhas erradas por minuto por endereço
  (`CF-Connecting-IP`), 50 pessoas por sala, mensagens até 1 MiB (o servidor
  próprio já tinha).
- Testado no navegador de verdade contra o Worker local (`wrangler dev`): entrar,
  lista, pedir a tela (o `watch` e o `view` chegam a quem transmite), layout de
  celular e o aviso de removido. Falta testar com vídeo de verdade, numa sala com
  alguém transmitindo pelo app.

**Fases do plano original** (cada uma testada antes da próxima, tudo na `dev`):

- [x] **W1. Núcleo sem Tauri.** `isTauri()` num lugar só; `log.ts` cai no
  console fora do app; tela cheia pelo `document.requestFullscreen` quando não
  é o app. Testes do `RoomClient` rodando como no navegador (sem
  `__TAURI_INTERNALS__`). Nada muda no app.
- [x] **W2. Tela de assistir na web.** Segunda entrada do Vite (`watch.html` +
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
- [x] **W3. Servidor e convite.**
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
- [x] **W4. Segurança.**
  - Limite de tentativas de senha por pessoa: atrás do túnel tudo chega de
    127.0.0.1, então contar pelo `CF-Connecting-IP` (ex.: 5 erradas por minuto,
    depois espera).
  - Limite de pessoas na sala e de tamanho das mensagens, se ainda não houver.
  - Conferir que a página não expõe nada além do que o app já expõe (só
    arquivos do frontend, sem segredo).
- [x] **W5. "No navegador" na lista.** O `join` ganha `client: "web"` (o
  servidor e as versões antigas ignoram campos que não conhecem); quem usa o
  app vê um ícone de navegador ao lado do nome, e o log diz "watching from a
  browser".
- [x] **W6. Testes.** Unidade (rotas, página do convite, limite de senha);
  ponta a ponta com o navegador de verdade: sala + `/watch` servidos localmente
  e uma aba assistindo uma transmissão (WebView e nativa), como nos testes do
  envio nativo. Chrome, Edge, Firefox (H.264 pelo OpenH264 que ele baixa) e, se
  der, Safari/iPhone e Android.
- [x] **W7. Lançar** (saiu na 0.7.0, 21/11/2026): junto com a seção 6 (fase S6, versão 0.7.0). No servidor
  próprio a página sai do Worker (arquivos estáticos), e não do PC do anfitrião.

**Limites (ficam como estão):**
- Só assistir. Transmitir pelo navegador daria (`getDisplayMedia`), mas pior
  que o app e sem som por programa; fica de fora.
- A mesma conectividade do app (só STUN): quem não conecta pelo app também não
  conecta pelo navegador. Isso é a seção 4.
- O link só funciona com o app do anfitrião aberto, como hoje.

---

## 3. Linux e Mac

> **Novembro de 2026: Mac e Linux saem juntos**, na mesma versão. O PR do Mac
> feito por um colaborador ([beqm/abretela#1](https://github.com/beqm/abretela/pull/1),
> Apple Silicon: ScreenCaptureKit, VideoToolbox, `.dmg`, CI e release em matriz)
> entra quando o Linux estiver pronto, não antes. Antes de juntar: resolver os
> conflitos com o que entrou na 0.7.0 (servidor próprio, navegador, TURN) e testar
> no Windows à mão, porque ele muda o `release.yml` e o atualizador.

O que o PR do Mac traz (Apple Silicon, macOS 14+): transmite pelo envio nativo e
assiste pelo WebView, como decidido abaixo.

- Captura e som: ScreenCaptureKit (`capture/mac/`). O sistema já entrega NV12
  no tamanho final; com a aceleração desligada, BGRA convertido no processador.
- Codificador: VideoToolbox (`encode/mac/`), com o controle de banda de baixa
  latência do chip. Medido num M5 Pro: 1080p60 a ~4 ms por frame.
- Sem `MediaStreamTrackGenerator` no WKWebView, o envio nativo é obrigatório
  (`src/lib/capture/support.ts`) e a prévia da própria tela vai por um canvas.
- Testado com `native_send_browser` e o WebKit assistindo: 1080p a 61 fps, 0%
  de perda, Opus estéreo, camadas diferentes por espectador.
- Falta: Macs Intel (binário universal), assinatura e notarização da Apple
  (sem elas o Gatekeeper bloqueia na primeira abertura e a permissão de
  gravação de tela pode ser pedida de novo a cada atualização), medir o
  WebView e a placa no `[perf]`, e testar em sala real com vários Macs.

O que o Linux traz (Ubuntu 24.04 com GNOME, no mesmo PR): transmite pelo envio
nativo e assiste pela recepção nativa.

- Assistir: o WebKitGTK não tem WebRTC (medido no 2.52), mas tem WebCodecs.
  O app recebe no Rust com o `str0m` (`receive/`, o espelho de `send/`), manda
  o H.264 e o Opus ainda codificados ao WebView por um WebSocket local, e o
  WebView decodifica (`src/lib/watch/decode.ts`). O H.264 depende do pacote
  `gstreamer1.0-libav`; sem ele o app avisa.
- Captura: xdg-desktop-portal (`ashpd`) com PipeWire (`capture/linux/`). Quem
  escolhe a tela é o sistema; o app não lista nem mostra miniaturas. Os frames
  vêm em BGRx na memória e são convertidos no processador.
- Codificador: OpenH264 no processador (`encode/linux/`), compilado junto com
  o app. Medido num núcleo ARM: 1080p a ~5 ms por frame.
- Som: o monitor da saída padrão do PipeWire, ou seja, o sistema inteiro.
- Pacote: `.deb`, com as dependências declaradas. O atalho do sistema passa
  o link `abretela://` ao app (modelo próprio em `templates/`), e o
  `cloudflared` embutido se chama `abretela-cloudflared`, para não brigar em
  `/usr/bin` com um que a pessoa já tenha.
- Testado sem tela, numa máquina virtual: `native_send_browser` assistido pelo
  Chromium (1080p a 60 fps, 0,03% de perda) e pela recepção nativa; a captura
  e o som contra um PipeWire de teste. Falta o teste com tela de verdade: a
  janela do portal, a bandeja, o link `abretela://`, a tela cheia.
- Falta: tirar o som do próprio AbreTela do que é transmitido (hoje quem
  transmite com som e assiste ao mesmo tempo devolve o som de quem ele
  assiste); codificar pela placa (VA-API); repasse (TURN) quando os dois
  lados são nativos (o `str0m` não tem cliente TURN); outras distribuições e
  X11; decidir a licença de patentes do H.264 ao compilar o OpenH264 junto
  (a da Cisco só cobre o binário que ela distribui); um AppImage (precisa
  levar os codecs junto, e ainda não foi gerado nem testado).

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
  | Linux | nativo (obrigatório) | nativo: o Rust recebe e o WebView decodifica com o WebCodecs |

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

### 4.4 TURN do anfitrião (decidido em novembro de 2026)

**Por quê:** o log de 19/11 mostra 32 tentativas falhas e 1 certa para assistir
o Davis. Os dois roteadores trocam a porta pública conforme o destino (o seu
vira 59231 → 2655/2679; o dele deu certo uma vez por uma porta nova, 1027).
Assim um lado não adivinha a porta do outro e a conexão direta só sai por
sorte. Só um repasse resolve.

**Como:** não há TURN fixo do app. Quem cria a sala configura o próprio TURN
em Configurações; os espectadores não configuram nada.

- Tipos (`src/lib/turn.ts`, um provedor por tipo atrás do mesmo contrato):
  - **Manual**: endereços, usuário e senha (coturn com senha fixa,
    ExpressTURN, Metered). Os espectadores recebem a mesma senha: aviso na tela.
  - **Cloudflare**: Key ID + API token. O app do anfitrião pede à API uma
    credencial temporária (24 h) e só ela vai para a sala.
  - **coturn com segredo** (`use-auth-secret`): o app do anfitrião calcula a
    credencial temporária (HMAC-SHA1) sem chamar nada.
- O segredo fica só no PC do anfitrião (não precisa de Worker). O
  `create_room` recebe os `iceServers` já prontos, o servidor da sala os guarda
  e manda no `welcome` (só depois da senha da sala). Versões antigas ignoram.
- Basta um lado ter rota pelo TURN. O str0m não tem cliente TURN, mas não
  precisa: o espectador (WebView) pega a rota de repasse e o envio nativo só
  manda para ela.
- Falhar ao gerar a credencial não impede criar a sala: vai só com STUN e o
  log avisa.
- **Senha que vence e é revogada** (feito em novembro de 2026): o anfitrião
  escolhe quanto a senha temporária dura (15 min a 24 h, padrão 1 h). Uns
  minutos antes de vencer o app gera outra e a sala manda para todos
  (`ice-servers`). Quem assiste por uma rota que passa pelo TURN pede rotas
  novas (`restart`) e quem transmite faz um ICE restart na mesma conexão, sem
  derrubar o vídeo (no envio nativo a conexão é refeita). Na Cloudflare a
  senha antiga é revogada 2 min depois da troca, e todas quando a sala ou o
  app fecham (`room/turn.rs`). O coturn não tem revogação: a senha só vence.
  Motivo: quem entra na sala recebe a senha e poderia gastar a franquia do
  anfitrião; assim uma senha vazada para de valer cedo.
- Depois: modo "dedicado" na criação da sala, onde o servidor dedicado entrega
  o TURN.

### 4.5 Passar a sala para outra pessoa

> **Novembro de 2026:** fica para depois. Com o servidor próprio (seção 6) a
> sala não depende de anfitrião, então quem precisa que a sala continue usa ele.
> Este plano vale só para o modo túnel, se um dia fizer falta.

Ao sair, o anfitrião escolhe quem vira o novo anfitrião, e a sala continua.

- Dá para fazer porque o vídeo não passa pelo servidor da sala: as conexões
  entre os PCs continuam vivas enquanto a sinalização troca de lugar.
- Passo a passo:
  1. O anfitrião escolhe a pessoa; a sala manda `host-request` para ela.
  2. O app dela abre servidor + túnel com a mesma senha e os mesmos limites
     de qualidade (e o TURN dela, se tiver) e espera o túnel responder.
  3. Ela responde `host-ready` com a URL nova; a sala antiga avisa todos
     (`moved`, com a URL) e fecha alguns segundos depois.
  4. Todos reconectam na URL nova com o mesmo nome e a mesma senha.
- **Ids estáveis:** hoje o servidor numera as pessoas (1, 2, 3...). Na sala nova
  cada um pede o id que tinha (`join` com `id`), para que as conexões abertas
  continuem valendo. Só cai a tela do anfitrião que saiu.
- Durante a troca (~5–45 s, o DNS do túnel novo demora) a interface mostra
  "trocando de anfitrião", sem fechar as telas.
- O link de convite muda (o túnel é do PC de cada um): o novo anfitrião vê o
  link novo para mandar. No modo dedicado isso não acontece.
- Só aparecem na lista quem tem a versão nova (o `join` diz a versão) e usa o
  app (quem assiste pelo navegador não pode hospedar).
- TURN: as conexões abertas pelo TURN antigo valem até a credencial vencer.
  Se o novo anfitrião não tiver TURN, a sala nova repassa os `iceServers` que
  já tinha (só credenciais temporárias ou as que já eram públicas na sala).
- Fora do escopo por enquanto: o anfitrião cair sem passar a sala (aí a sala
  acaba, como hoje).

---

## 6. Servidor próprio (decidido em novembro de 2026)

**Por quê:** um servidor de salas do próprio app (um Worker na conta do dono
do projeto) resolveria passar o anfitrião, o link fixo e a página do navegador,
mas a conta seria de quem mantém o app, e cresce com o app. Em vez disso, **cada
pessoa pode ter o próprio servidor, na própria conta da Cloudflare**, montado e
administrado só pelo app. Quem não quiser continua com o túnel, como hoje.

### Os dois modos de sala

| | Túnel (padrão, como hoje) | Servidor próprio |
|---|---|---|
| Onde a sala vive | no PC do anfitrião, pelo túnel | num Worker da conta do dono |
| Anfitrião | quem criou; se sai, a sala acaba | não existe: a sala fica de pé sozinha |
| Link | muda a cada sala | fixo, por sala |
| TURN | o do anfitrião (seção 4.4), senha da sala toda | embutido no servidor, senha por pessoa |
| Kikar | o anfitrião, pelo app | o dono, pelo app; revoga o TURN da pessoa |
| Navegador | a página sai do PC do anfitrião | a página sai do Worker |
| Custo | nenhum | o da conta do dono (plano grátis, abaixo) |

Os dois ficam atrás do mesmo contrato no app (onde a sala vive), como o túnel já
está hoje (`tunnel::Tunnel`), e falam o mesmo protocolo de sala (`join`,
`welcome`, `peer-*`, `signal`, `ice-servers`), mais o que for de administração.

### Servidor = uma sala, e a lista de servidores no app

- **Cada servidor é uma sala só**, a mesma tela de hoje (quem está, quem está ao
  vivo, assistir). Não há canais dentro do servidor.
- **Tela inicial com a barra de servidores do Discord:** ícones à esquerda
  (iniciais do nome), um clique para entrar; passar o mouse mostra o nome e
  quantas pessoas e quantas ao vivo; selo vermelho com quantos ao vivo e coroa
  nos que a pessoa administra; botão direito para copiar o link ou tirar da
  lista. Cabe uma lista grande (rola). No meio, as ações de hoje: criar uma
  sala pelo túnel e entrar por um link.
- **Senha:** o link do servidor não leva a senha (no túnel continua levando), e
  o app **pede a senha toda vez** (decidido em novembro de 2026, por
  segurança): a lista guarda só o endereço e o nome. O dono entra pela chave de
  administrador, sem senha.
- **Quem é o dono:** a chave de administrador (abaixo), nunca o nome, o IP ou a
  conta da Cloudflare. O app do dono manda a chave ao conectar; o Worker compara
  com o hash que guarda e marca aquela conexão como dona. Depois: chaves de
  moderador, com menos poderes.

### Montar o servidor pelo app, sem abrir a Cloudflare

O dono cola **um token da API da Cloudflare** em Configurações e o app faz o
resto. Depois disso não precisa voltar ao painel da Cloudflare para nada.

- **Criar o token:** o app abre o painel já com o formulário preenchido
  (`dash.cloudflare.com/profile/api-tokens?permissionGroupKeys=...&name=AbreTela`).
  Permissões: Workers Scripts (editar), Account Settings (ler) e Realtime
  (editar, para a chave do TURN). O link preenche `workers_scripts` e
  `account_settings`; para o Realtime não há chave curta documentada, então a
  tela explica como marcar essa permissão. Conferir os nomes ao implementar.
- **O que o app faz com o token** (API REST da Cloudflare, sem wrangler):
  1. `GET /accounts`: acha a conta (se houver mais de uma, pergunta qual).
  2. `GET /accounts/{id}/workers/subdomain`: o `<conta>.workers.dev`; se não
     existir, cria.
  3. `POST /accounts/{id}/calls/turn_keys`: cria a chave do TURN. A resposta traz
     `uid` (o Key ID) e `secret` (o token), e o `secret` só aparece nessa hora.
  4. Sobe os arquivos da página do navegador (`assets-upload-session`, depois
     `workers/assets/upload`, que devolve o token de conclusão).
  5. `PUT /accounts/{id}/workers/scripts/abretela`: o código do servidor
     (multipart, `main_module`, `compatibility_date`), a classe da sala como
     Durable Object com SQLite (`new_sqlite_classes`; o plano grátis só aceita
     SQLite), os segredos (`secret_text`: chave do TURN e o hash da chave de
     administrador) e os arquivos do passo 4.
  6. `POST .../scripts/abretela/subdomain`: liga o endereço `workers.dev`.
- O código do servidor vai dentro do app, já empacotado (esbuild no build). Cada
  versão do app sabe a versão do servidor que espera; se a do Worker for mais
  velha, o app oferece atualizar (passo 5 de novo).

### Onde fica cada segredo

- **Token da Cloudflare:** só no PC do dono, no Gerenciador de Credenciais do
  Windows (não no `localStorage`). Nunca vai para o Worker nem para a sala.
- **Chave do TURN:** só no Worker, como segredo. Nem o app guarda.
- **Chave de administrador:** o app gera ao montar o servidor e guarda no
  Gerenciador de Credenciais; o Worker guarda só o hash. É com ela que o app do
  dono se identifica para administrar.
- **Outro PC do dono:** cola o token de novo; o app acha o servidor que já
  existe e troca a chave de administrador (`PUT .../secrets`).

### O que o dono faz pelo app

- Criar mais de um servidor se quiser (cada um é um Worker e uma sala);
  renomear; mudar senha e limites de qualidade; copiar o link.
- Ver quem está em cada sala, kikar e banir (por sessão; o banido não volta com
  o mesmo link enquanto o ban durar).
- Escolher quanto dura a senha do TURN (como na seção 4.4).
- Atualizar o servidor quando o app pedir.
- **Apagar o servidor:** `DELETE .../workers/scripts/abretela?force=true` (apaga
  as salas e os dados delas) e `DELETE .../calls/turn_keys/{uid}`. Pede
  confirmação, porque não tem volta.

### Como o servidor funciona

- Um Worker que recebe as conexões e um Durable Object com a sala. Ela guarda
  no SQLite o nome, o hash da senha, os limites e os banidos; quem está
  conectado fica só na memória.
- **Hibernação obrigatória** (API de hibernação de WebSocket): sala parada não
  gasta tempo. O app manda um `ping` de texto e o Worker responde pela resposta
  automática (`setWebSocketAutoResponse`), que não acorda a sala. O servidor
  não manda pings, ao contrário do servidor do túnel.
- **TURN por pessoa:** quem entra com a senha certa recebe uma credencial só
  dela; um alarme do Durable Object manda outra antes de vencer
  (`ice-servers`); ao sair ou ser kikada, a dela é revogada. O resto da troca
  (ICE restart na mesma conexão) é o que já existe para a seção 4.4.
- **Link:** `https://<nome>.<conta>.workers.dev`, sem senha. As versões antigas
  esperam a senha no link, então o servidor fica só para a versão nova em
  diante.
- **Contra abuso:** só o dono cria salas; limite de tentativas de senha por
  `CF-Connecting-IP`; limite de pessoas por sala e de tamanho de mensagem.

### Plano grátis da Cloudflare (páginas de 13 a 19/11/2026)

- Workers: 100 mil requisições por dia; 10 ms de processador por requisição.
- Durable Objects (só com SQLite): 100 mil requisições por dia e 13.000 GB-s de
  tempo por dia (~28 h de uma sala acordada). Abrir um WebSocket conta 1
  requisição; mensagens recebidas contam 20 por 1; enviadas e pings não contam.
- SQLite: 5 milhões de linhas lidas e 100 mil gravadas por dia; 5 GB no total.
- Arquivos estáticos (a página do navegador): grátis e sem limite.
- **Passou do limite, para de funcionar até zerar, às 00:00 UTC (21:00 em
  Brasília).** Não cobra. O app precisa dizer isso claramente quando acontecer e
  sugerir o túnel até lá.
- O que pode gerar conta é o TURN (1.000 GB por mês grátis, depois cobra por
  uso). Para um grupo de amigos, o resto sobra com folga.

### Fases (cada uma testada antes da próxima, tudo na `dev`)

- [x] **S1. Servidor** (feito em novembro de 2026). Pasta `server/`: o Worker
  (`index.ts`), a sala como Durable Object (`room.ts`) e a lógica sem nada da
  Cloudflare (`core.ts`, testada no Node com WebSockets, banco, relógio e TURN
  falsos). Mesmo protocolo do túnel, mais: `welcome` com `owner`, `room` e
  `version`; `kick` (só o dono); `kicked`, `room` e `settings` do servidor
  para a sala; erros `not-configured`, `banned`, `too-many-attempts` e
  `room-full`. Administração por HTTP com a chave (`/api/admin/room`, `kick`,
  `unban`) e `/api/info` público. Ban pelo hash do endereço, por tempo. Senha
  da sala com PBKDF2. Teste de ponta a ponta com o `wrangler dev`:
  `node server/scripts/smoke.mjs <url> <chave>`. Para rodar localmente, a
  chave de teste vai em `server/.dev.vars` (`ADMIN_KEY_HASH=<sha256>`, fora do
  git).
- [x] **S2. Montar pelo app** (feito em novembro de 2026; falta testar com uma
  conta de verdade). `scripts/build-server.mjs` empacota o Worker (esbuild) em
  `server/dist/worker.js`, que o app embute; roda no `npm run dev` e no
  `npm run build`. No Rust, `src-tauri/src/selfhost/`: `cloudflare.rs` (a API
  REST: contas, subdomínio, chave do TURN, subir, ligar o `workers.dev`,
  segredos, listar e apagar Workers), `secrets.rs` (token e chaves de
  administrador no Gerenciador de Credenciais, pelo `keyring`) e os comandos
  `selfhost_*`. O upload segue o formato do wrangler: na primeira vez
  `migrations: {new_tag: "v1", steps: [{new_sqlite_classes: ["Room"]}]}` e os
  segredos como `secret_text`; nas atualizações, sem migração e com
  `keep_bindings`. Sem a permissão do Realtime o servidor sai sem TURN (e
  avisa); se algo falhar no meio, desfaz o que criou. Se o `workers.dev` novo
  demorar a responder, a senha fica para depois ("Definir senha"). Tela nova
  em Configurações > Servidor (`ServerSettings.svelte`): token, conta, seus
  servidores (pessoas e ao vivo, copiar link, atualizar, apagar com
  confirmação), novo servidor e assumir um servidor montado em outro PC. A
  página do navegador (arquivos estáticos) fica para a S6.
- [x] **S3. Lista de servidores e entrar** (feito em novembro de 2026). Entrar
  é o mesmo `RoomClient` nos dois modos: o servidor fala o protocolo do túnel e
  o link vira `wss://<servidor>/ws`. A tela inicial tem só o campo do link (a
  senha que faltar vem no `PasswordModal`) e, abaixo, a lista "Seus servidores"
  (`ServerRail.svelte`, `lib/servers.ts`), uma barra de ícones à esquerda: os
  que este PC administra (coroa; entra como dono pela chave de administrador,
  `selfhost_admin_key`) e os em que já entrou (`joinedServers`, só endereço e
  nome; a senha é pedida toda vez), com pessoas e ao vivo pelo `/api/info` a
  cada minuto. Na sinalização: `ping` a cada 30 s
  (`PING_MS`; o túnel ignora), recusas do servidor (`banned`,
  `not-configured`, `too-many-attempts`, `room-full`) viram `RoomRefusedError`
  e não tentam de novo, `join` com `admin`. O `RoomClient` guarda `server`
  (nome, dono, versão) e `kicked`, e aplica `settings` e `room` na hora. Na
  sala: nome do servidor no topo, convite sem a senha, e avisos de "removido
  da sala" e "a conexão com o servidor caiu". Testes do `RoomClient` contra a
  lógica de verdade do servidor (`room.server.test.ts`). O "contrato onde a
  sala vive" ficou só do lado de criar (túnel): entrar não precisa dele.
- [x] **S4. Administração** (feito em novembro de 2026). Em Configurações >
  Servidor, "Administrar" abre o painel do dono no cartão do servidor
  (`ServerAdmin.svelte`, `lib/serverAdmin.ts`, pela API do Worker com a chave
  deste PC): nome, nova senha, limites de qualidade, duração da senha do TURN,
  quem está na sala (dono marcado; "Ações": remover, ou banir por 1 hora, 1 dia
  ou 1 semana) e os banidos (desbanir). Na sala, o dono tem o mesmo menu em cada
  pessoa (botão de ações ou botão direito), pelo `kick` do WebSocket
  (`RoomClient.kick`); todos veem a coroa no nome do dono. O menu do botão
  direito virou um componente (`ui/ContextMenu.svelte`), usado também pela
  barra de servidores.
- [x] **S5. Kikar no túnel** (feito em novembro de 2026). O servidor da sala do
  túnel (`room/server.rs`) gera uma chave de anfitrião em cada sala, que o
  `create_room` devolve só para o app de quem criou (`hostKey`); o app entra com
  ela (`join` com `admin`) e vira o dono da sala: `owner` no `welcome` e nas
  pessoas (coroa, "anfitrião"), e o mesmo `kick` do servidor próprio (remover ou
  banir). O ban é pelo endereço que o cloudflared repassa (`CF-Connecting-IP`) e
  vale enquanto a sala existir. A senha do TURN continua sendo da sala inteira
  nesse modo: quem é kikado guarda a dela até vencer. No app, `RoomClient.owner`
  vale para os dois modos.
- [x] **S6. Assistir pelo navegador** (feito em novembro de 2026; detalhes na
  seção 5). A página vai num arquivo só (`dist-web/watch.html`), embutido no
  Worker (servido em `/`) e no app (o túnel serve em `/watch`). A barra de
  servidores do app passou a perguntar quem está em cada servidor só com a
  janela em foco (economiza o limite grátis de quem tem o servidor).
- [x] **S7. Lançar**: saiu na 0.7.0 (21/11/2026). Depois dela, na `dev`: a página do
  navegador põe "Abrir no AbreTela" primeiro e lembra quem tem o app (servidor versão 4).

---

## 7. Compartilhar a webcam

Além de telas e janelas, compartilhar a câmera, **ao mesmo tempo que a tela ou
sozinha** (ex.: o jogo numa tela e o rosto em outra). Vale nos dois modos de
sala e não depende da seção 6.

**Como:**
- **Escolher:** o seletor de "compartilhar" ganha a aba **Câmera**, com as
  câmeras do PC (`enumerateDevices`), prévia ao vivo e qualidade dentro dos
  limites da sala. Microfone fica de fora por enquanto (o som que já vai é o do
  PC, junto da tela).
- **Capturar e enviar pelo WebView** (`getUserMedia` e o WebRTC dele), sem
  captura nativa: funciona no WebView2 e no WKWebView do Mac. Com o envio nativo
  ligado, a tela sai pelo Rust e a câmera continua pelo WebView.
- **Duas transmissões por pessoa:** hoje cada pessoa tem uma só (`sharing` +
  `label`). Passa a ter até duas, `screen` e `camera`, cada uma com o próprio
  rótulo. As mensagens de assistir (`watch`, `offer`, `answer`, `ice`, `view`,
  `restart`) ganham `source`, e cada par (pessoa, fonte) tem a própria conexão.
  Versões antigas ignoram os campos novos e continuam vendo só a tela.
- **Na sala:** a câmera aparece como outra tela ("Câmera de Ana"), com prévia,
  qualidade pelo tamanho em que está sendo vista (o `view` de hoje), tela cheia
  e janela flutuante. Depois, se fizer falta: câmera por cima da tela da mesma
  pessoa.
- **Permissão:** o Windows e o Mac pedem permissão de câmera. Conferir como o
  Tauri trata o pedido do WebView em cada sistema, e o que mostrar quando a
  pessoa negar.
- **Quem assiste pelo navegador** vê as câmeras como vê as telas. Mandar a
  câmera pelo navegador seria fácil (`getUserMedia` existe lá), mas fica de fora
  por enquanto: transmitir continua sendo pelo app.

**Fases:**
- [ ] **C1. Protocolo de duas fontes** (`source` nas mensagens, `camera` no
  estado da pessoa) com testes, sem mudar nada na tela.
- [ ] **C2. Câmera no seletor e na sala**, com permissão e prévia.
- [ ] **C3. Teste numa sala real** com tela e câmera ao mesmo tempo, incluindo
  alguém numa versão antiga.

---

## Caso do Eliaga (resolvido em novembro de 2026)

Ninguém conseguia assistir a tela dele, e ele assistia todo mundo. O log com o
diagnóstico de rede mostrou que os roteadores dos dois lados trocavam a porta
pública conforme o destino (NAT simétrico), então a conexão direta não saía. O
TURN do anfitrião (seção 4.4) resolveu: com o repasse, a tela dele abre.

---

## Outras ideias levantadas (sem ordem)

- **TURN** para quem não conecta direto: feito (seção 4.4 e servidor próprio).
- Mostrar **quem está te assistindo** enquanto você transmite.
- **Notificação do Windows** quando alguém abre a tela, com clique para assistir.
- **Atalhos globais** (ex.: abrir/fechar sua tela sem sair do jogo).
- Chat de texto ou reações leves na sala.
- Janela flutuante própria (título "Nome - AbreTela") no lugar do
  Picture-in-Picture do navegador, que mostra `tauri.localhost` no topo.
- Indicador de fps e atraso reais em cada tela.
- Mensagens de erro mais claras (túnel, captura).
- Anfitrião poder remover alguém da sala: entrou na seção 6 (S4 e S5).
- SFU (Cloudflare Realtime ou LiveKit) só se as salas crescerem além de ~10
  pessoas transmitindo ao mesmo tempo.
