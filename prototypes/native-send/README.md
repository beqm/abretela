# Protótipo: envio nativo (ROADMAP, seção 2)

Prova de conceito isolada (não entra no app): o Rust codifica H.264 **uma vez**
e manda por WebRTC para todos os navegadores que abrirem a página. Quem
assiste usa o WebRTC normal do Chromium, como o WebView2 do app.

```bash
cargo run --release                         # abrir http://<ip>:3010 em várias abas
LIMIT_KBPS=300 cargo run --release          # simula rede de 300 kbps nos espectadores pares
```

- `src/main.rs`: vídeo de teste (barras que andam, 1280x720 a 30 fps),
  codificador openh264 no processador, str0m para o WebRTC, um laço por
  espectador e o codificador usando a menor estimativa de banda entre eles.
- `src/viewer.html`: aceita a oferta (quem transmite oferece, como no app) e
  mostra as estatísticas do vídeo recebido.

## Decisão: **str0m**

Comparação feita em novembro de 2026:

| | str0m 0.25 | libwebrtc (LiveKit `rust-sdks`) | webrtc-rs |
|---|---|---|---|
| Situação | estável, lançamentos frequentes | estável | reescrita em andamento (0.20 "sans-I/O" em versão candidata) |
| Recebe frames **já codificados** | sim (`Writer::write`) | não: quer frames crus e codifica sozinho | sim |
| Placa de vídeo no Windows | por nossa conta (Media Foundation), o que já era o plano | binários prontos **sem** codificação pela placa no Windows (PR aberto e parado) | por nossa conta |
| Codificar uma vez para todos | natural | difícil (um codificador por conexão) | natural |
| Controle de banda | TWCC + estimativa; as políticas (quadro-chave, camadas) ficam conosco | o do Chromium, o melhor | menos maduro |
| Tamanho / compilação | puro Rust; criptografia nativa do Windows (`wincrypto`), Mac (`apple-crypto`) ou Rust puro no Linux | C++ grande, binário pré-compilado por sistema | puro Rust |
| Executável do protótipo | **5 MB** com str0m + openh264 | — | — |

Por que str0m: o plano já era codificar pela placa com Media Foundation e
mandar o mesmo frame para todos. A libwebrtc briga com isso (quer codificar
ela mesma, uma vez por espectador, e no Windows sem a placa). O webrtc-rs está
no meio de uma reescrita. O str0m aceita o frame pronto, roda em qualquer
runtime (é síncrono, "sans-I/O"), compila nos três sistemas sem C++ e usa a
criptografia do próprio sistema.

O preço: o str0m entrega a **estimativa** de banda, mas as **decisões** que a
libwebrtc toma sozinha passam a ser nossas (ver abaixo).

## O que o protótipo mostrou

Testado neste PC com o navegador do app (Chromium) em duas abas.

**Compatibilidade.** Funcionou sem ajuste do lado de quem assiste: H.264
Constrained Baseline (`profile-level-id=42e01f`, `packetization-mode=1`),
1280x720 a 30 fps, 0 pacotes perdidos. ICE + DTLS conectados em ~2–4 ms na
rede local. Oferta feita pelo Rust, resposta pelo navegador, como no app.

**Uma codificação para todos.** Dois espectadores recebendo o mesmo fluxo de
um codificador só, cada um a 30 fps, sem perda.

**Controle de banda (rede simulada de 300 kbps para um espectador).**

- A estimativa reage rápido: 1500 → 191 kbps em ~3 s, e o codificador
  acompanha (usa a menor estimativa entre os espectadores).
- **Primeira tentativa (piso de 300 kbps, sem limitar quadros-chave): espiral.**
  O espectador com perda pediu 674 quadros-chave (PLI), o piso ficou acima da
  rede, a estimativa caiu até 0 e nunca voltou. Os quadros-chave pedidos por ele
  também saíam para o espectador saudável.
- **Com piso de 100 kbps e no máximo 1 quadro-chave por segundo:** converge.
  A estimativa oscila entre ~110 e ~370 kbps em volta do limite, a perda cai de
  ~30% para 0–2% em ~15 s e os quadros-chave caem para 17.
- **Custo de codificar uma vez:** o espectador saudável foi puxado junto
  (~290 kbps, 14 fps), porque o codificador segue o mais lento.

**Processador.** O openh264 levou 12–19 ms por frame em 720p. Para 1080p60
(16 ms por frame) não dá no processador: o codificador pela placa (Media
Foundation) é obrigatório, como já estava no plano.

## O que fica para a implementação (seção 2)

1. **Codificador pela placa** (Media Foundation: NVENC/AMF/QuickSync) a partir
   da textura D3D11 da captura, saindo H.264 Constrained Baseline (ou High com
   fallback, conferindo o que o outro lado aceita).
2. **Política de quadro-chave**: no máximo 1 por segundo, juntando os pedidos.
3. **Não deixar um espectador lento derrubar os outros.** Opções, da mais
   simples para a mais completa:
   - pausar o vídeo para quem está abaixo de um mínimo (como já fazemos quando
     o app está escondido) e avisar;
   - 2–3 qualidades (simulcast, que o str0m suporta, ou um segundo codificador
     menor) e cada espectador recebe a que cabe na banda dele.
4. **Piso de banda abaixo do que a rede aguenta** e reduzir fps antes de
   resolução para tela (texto nítido importa mais que movimento).
5. **Rede**: ICE completo com STUN (o protótipo só usa a rede local), e as
   mensagens da sala (offer/answer/ice) iguais às de hoje, para quem assiste
   pelo WebView não perceber diferença.
6. Áudio (Opus) entra igual, numa segunda seção de mídia.

Fontes: [str0m](https://github.com/algesten/str0m),
[LiveKit rust-sdks](https://github.com/livekit/rust-sdks),
[webrtc-rs 0.20 rc](https://webrtc.rs/blog/2026/06/30/webrtc-v0.20.0-rc.1-toward-stable-async-webrtc).
