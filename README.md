<p align="center">
  <img src="app-icon.svg" width="96" alt="">
</p>

<h1 align="center">AbreTela</h1>

<p align="center">Compartilhamento de tela entre amigos, com áudio, para Windows.</p>

---

O AbreTela permite que um grupo de pessoas transmita e assista às telas umas das outras ao mesmo tempo: jogos, aplicativos ou a tela inteira, com o áudio junto. Uma pessoa cria a sala e envia o link de convite; as demais entram com um clique. Não é necessário criar conta nem configurar servidor ou roteador.

## Instalação

1. Acesse a página de **[Releases](https://github.com/beqm/abretela/releases/latest)** e baixe o arquivo `AbreTela_..._x64-setup.exe`.
2. Execute o instalador.
3. As atualizações seguintes são instaladas automaticamente ao abrir o app.

Compatível com Windows 10 e Windows 11.

> [!WARNING]
> **Aviso do Windows na instalação**
>
> Ao abrir o instalador, o Windows pode exibir a mensagem *"O Windows protegeu o computador"*. Isso acontece porque o instalador ainda não possui assinatura digital, e não indica um problema no arquivo. Para continuar, clique em **Mais informações** e depois em **Executar assim mesmo**.

## Como usar

1. **Primeira abertura:** informe o nome que será exibido para os outros participantes. Ele pode ser alterado depois em **Configurações > Geral > Perfil**.
2. **Criar uma sala:** clique em **Criar sala**. Quem cria a sala é o anfitrião.
3. **Convidar:** clique em **Copiar convite** e envie o link por Discord, WhatsApp ou qualquer outro canal. O link já inclui a senha da sala e abre o AbreTela diretamente nela. Quem ainda não tem o app encontra o botão de download na própria página do convite.
4. **Transmitir:** clique em **Abrir minha tela** e escolha um aplicativo ou uma tela inteira. Ao escolher um aplicativo, apenas o áudio dele é transmitido. Durante a transmissão, **Trocar tela** muda o conteúdo sem desconectar quem está assistindo.
5. **Assistir:** clique em **Assistir** ao lado de quem está ao vivo. É possível acompanhar várias transmissões em grade, dar foco a uma delas, abrir em tela cheia ou em uma janela flutuante que fica sobre outros aplicativos. Ao passar o mouse sobre um participante, é exibida uma prévia da tela dele.

### Recomendações

- A sala permanece disponível enquanto o app do anfitrião estiver aberto. Se o anfitrião sair, a sala é encerrada para todos.
- As opções da sala (botão ao lado de **Criar sala**) permitem definir uma senha própria e os limites de qualidade da transmissão.
- Para reduzir o impacto em jogos, o app envia uma versão menor da tela para quem a vê em miniatura e não envia vídeo para quem está com o AbreTela minimizado.
- Se a transmissão aparecer com a tela preta, desative **Aceleração de hardware** em **Configurações > Geral**.

## Como a conexão funciona

O AbreTela não depende de servidores próprios. A conexão é formada por duas partes:

1. **Túnel da Cloudflare (ponto de encontro).** Ao criar uma sala, o app inicia um pequeno servidor no computador do anfitrião e o publica na internet por meio de um [túnel gratuito da Cloudflare](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/do-more-with-tunnels/trycloudflare/) (`cloudflared`, incluído no app). O túnel gera um endereço aleatório no formato `https://...trycloudflare.com`, que é o link do convite. Por ele trafegam apenas mensagens de controle: quem entrou na sala, quem está ao vivo e os dados necessários para os computadores se conectarem.
2. **Conexão direta (WebRTC).** O vídeo e o áudio são enviados diretamente entre os computadores dos participantes, sem passar pela Cloudflare nem por qualquer servidor do AbreTela.

> [!IMPORTANT]
> **Pontos de atenção sobre o túnel e a conexão**
>
> - **O link só funciona enquanto a sala estiver aberta.** Cada nova sala recebe um endereço diferente, e links antigos deixam de funcionar.
> - **A senha não é enviada a nenhum servidor.** Ela fica após o `#` do link, parte do endereço que os navegadores não transmitem.
> - **O túnel é um serviço gratuito da Cloudflare**, sem garantia de disponibilidade. Em caso de instabilidade do serviço, a criação de salas pode falhar temporariamente.
> - **Quem transmite consome upload para cada pessoa que assiste.** Uma transmissão em 1080p a 60 fps usa cerca de 7 Mbps por espectador.
> - **Redes muito restritas** (algumas redes corporativas ou universitárias) podem impedir a conexão direta entre os computadores. Nesses casos, a sala abre normalmente, mas a transmissão permanece em "Conectando...".

## Licença

Distribuído sob a licença [MIT](LICENSE).
