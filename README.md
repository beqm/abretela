<p align="center">
  <img src="app-icon.svg" width="96" alt="">
</p>

<h1 align="center">AbreTela</h1>

<p align="center">Abra sua tela para os amigos: jogos, apps ou a tela inteira, com áudio.</p>

---

O AbreTela é um app para Windows feito para assistir à tela dos amigos, todo mundo ao mesmo tempo. Uma pessoa cria a sala, manda o link e cada um abre a tela que quiser. Não tem conta, cadastro nem servidor para configurar.

## Baixar

1. Vá em **[Releases](https://github.com/beqm/abretela/releases/latest)** e baixe o `AbreTela_..._x64-setup.exe`.
2. Abra o instalador. Se o Windows mostrar "O Windows protegeu o computador", clique em **Mais informações** e depois em **Executar assim mesmo** (isso aparece porque o app ainda não tem assinatura digital paga).
3. Pronto. O app se atualiza sozinho quando sai uma versão nova.

Funciona no Windows 10 e 11.

## Como usar

**Criar uma sala**
Digite seu nome e clique em **Criar sala**. Em alguns segundos a sala está no ar.

**Chamar os amigos**
Clique em **Copiar convite** e mande o link no Discord, WhatsApp ou onde quiser. Quem clicar no link abre o AbreTela direto na sala (quem ainda não tem o app vê o botão de download). O link já leva a senha.

**Abrir sua tela**
Clique em **Abrir minha tela** e escolha um aplicativo (por exemplo, o jogo) ou uma tela inteira. O áudio vai junto: ao escolher um app, só o som dele é transmitido.

**Assistir**
Clique em **Assistir** ao lado de quem está ao vivo. Dá para ver várias telas em grade, clicar em uma para dar foco, dar dois cliques para tela cheia ou abrir numa **janela flutuante** que fica por cima do jogo.

### Dicas

- **Quem cria a sala é o anfitrião**: a sala fica aberta enquanto o app dele estiver aberto. Se ele sair, a sala acaba para todos.
- Nas opções da sala (o botão ao lado de **Criar sala**) dá para definir uma senha própria e a qualidade permitida.
- O app se ajusta para pesar pouco no seu jogo: quem te vê em miniatura recebe uma versão menor da sua tela, e quem está com o AbreTela minimizado não recebe vídeo.
- Se a sua transmissão ficar com tela preta, desligue **Aceleração de hardware** em Configurações.

## Como a conexão funciona

Ninguém precisa abrir portas no roteador nem configurar nada. Por baixo, são duas partes:

1. **O ponto de encontro (túnel da Cloudflare).** Ao criar a sala, o app liga um pequeno servidor no seu PC e o publica na internet com um [túnel gratuito da Cloudflare](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/do-more-with-tunnels/trycloudflare/) (o `cloudflared`, que já vem dentro do app). Esse túnel gera um endereço `https://...trycloudflare.com` aleatório, que é o link do convite. Por ele passam só mensagens pequenas: quem entrou, quem está ao vivo e os dados para os PCs se encontrarem.
2. **O vídeo e o áudio vão direto entre os PCs (WebRTC).** Depois que dois PCs se encontram pelo túnel, a transmissão vai de um para o outro, sem passar pela Cloudflare nem por nenhum servidor do AbreTela.

Algumas consequências disso:

- **A senha nunca sai do link.** Ela fica depois do `#`, e essa parte do endereço não é enviada a nenhum servidor.
- **O link só funciona enquanto a sala está aberta.** Cada sala nova tem um endereço novo.
- **Quem transmite usa upload para cada pessoa que assiste.** Uma tela em 1080p 60fps usa uns 7 Mbps por pessoa, por isso o app reduz a qualidade para quem está te vendo pequeno.
- Em redes muito fechadas (algumas redes de empresa ou faculdade), os PCs podem não conseguir se conectar direto. Nesses casos a sala abre, mas a tela fica em "Conectando...".

## Para desenvolvedores

Feito com [Tauri 2](https://tauri.app) (Rust) e Svelte 5 + TypeScript.

Requisitos: Node.js, Rust (toolchain MSVC) e Visual Studio Build Tools.

```sh
npm install
npm run fetch-cloudflared   # baixa o cloudflared para src-tauri/binaries/
npm run tauri dev           # modo desenvolvimento
npm run tauri build         # gera o instalador em src-tauri/target/release/bundle/nsis/
```

Com `npm run dev`, a URL `http://localhost:1420/?mock` abre uma sala falsa no navegador para mexer na interface sem Tauri.

### Onde fica cada coisa

| Arquivo | O que faz |
|---|---|
| `src-tauri/src/server.rs` | Servidor da sala: sinalização e a página do convite |
| `src-tauri/src/invite_page.rs` | Página que o link do convite abre no navegador |
| `src-tauri/src/tunnel/` | Túnel para a internet. `mod.rs` é o contrato; `cloudflare.rs` é a implementação atual |
| `src-tauri/src/capture.rs` | Captura nativa de vídeo (Windows Graphics Capture) e áudio por app (WASAPI) |
| `src-tauri/src/gpu_convert.rs` | Redimensiona e converte a captura na placa de vídeo (D3D11) |
| `src/lib/room.ts` | Cliente da sala, conexões WebRTC e leitura de convites |
| `src/lib/capture.ts` | Transforma os frames nativos em vídeo e áudio para o WebRTC |

## Licença

[MIT](LICENSE)
