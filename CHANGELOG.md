# Changelog

## [Não lançado]

### Novidades
- **Versão para Mac**: o AbreTela agora roda em Macs com chip da Apple (macOS 14 ou mais novo). Dá para criar salas, assistir e transmitir telas e janelas com som, junto com quem está no Windows.
  - No Mac a tela é sempre codificada pelo próprio app (o "envio nativo" do Windows), pelo chip.
  - Para transmitir, o macOS pede a permissão de gravação de tela; o app avisa e leva até o lugar certo dos Ajustes.
  - O app ainda não é assinado pela Apple: veja no README como abrir pela primeira vez.
- **Versão para Linux**: o AbreTela agora roda no Ubuntu 24.04 com GNOME. Dá para criar salas, assistir e transmitir com som, junto com quem está no Windows e no Mac.
  - Quem escolhe a tela ou a janela a transmitir é o próprio sistema, numa janela dele.
  - O som transmitido é o do sistema inteiro, com o do próprio AbreTela junto: quem transmite com som e assiste alguém ao mesmo tempo devolve a essa pessoa o som dela.
  - A tela é codificada no processador (OpenH264). Para assistir, o sistema precisa do pacote `gstreamer1.0-libav`, que o `.deb` já pede.
  - Vem em `.deb`.
- A página do convite mostra o botão de download do sistema de quem abriu o link.
- **Senha do servidor só na primeira vez**: ao entrar num servidor próprio de outra pessoa, o app pede a senha uma vez e lembra dela no cofre do sistema (Gerenciador de Credenciais do Windows, Chaves do Mac ou chaveiro do Linux). Se o dono trocar a senha, o app pede de novo; tirar o servidor da lista apaga a senha guardada.

### Correções
- **Som estourando para quem assiste** em jogos altos (freada, motor, explosão): o som de um app é capturado antes do volume e do limitador do Windows e podia passar do máximo, e quem assistia ouvia cortado. Agora o app abaixa o volume só nesses picos, igual ao sistema faz para quem transmite. Vale para o envio nativo e o pelo WebView.

### Por dentro
- O que é de cada sistema fica em pastas próprias (`capture/mac`, `capture/linux`, `encode/mac`, `encode/linux`, `logging/unix`); nada muda no Windows.
- No Linux o WebView (WebKitGTK) não tem WebRTC: o app recebe a tela dos outros no Rust (`receive/`, o espelho do envio nativo) e o WebView só decodifica e mostra.
- Os testes automáticos e a publicação das versões rodam no Windows, no macOS e no Ubuntu.
## [0.7.0] - 2026-11-21

### Novidades
- **Servidor próprio**: uma sala que fica no ar sozinha, na sua conta grátis da Cloudflare, com link fixo. Ela não depende de ninguém estar com o app aberto: quem tem a senha entra quando quiser.
  - Tudo pelo app, em Configurações > Servidor: você cola um token da Cloudflare e o app monta o servidor. Depois não precisa abrir a Cloudflare para nada.
  - O repasse (TURN) vem embutido: cada pessoa que entra recebe uma senha só dela, trocada sozinha antes de vencer e cancelada quando ela sai.
  - A senha da sala é pedida toda vez que alguém entra (o link nunca leva a senha).
  - Você pode ter mais de um servidor, atualizar quando o app avisar, passar a administrar de outro PC e apagar tudo quando quiser.
- **Painel do dono**: em Configurações > Servidor > Administrar, mude o nome, a senha, a qualidade permitida e quanto dura a senha do repasse; veja quem está na sala; remova ou bana alguém por 1 hora, 1 dia ou 1 semana; tire o ban.
- **Remover e banir pela sala**: o dono do servidor e o anfitrião de uma sala comum têm, em cada pessoa da lista, a opção de tirar da sala (com ou sem ban). Todos veem uma coroa no nome de quem administra.
- **Assistir pelo navegador**: quem recebe o link e não tem o app assiste pelo navegador, sem instalar nada, inclusive no celular. Na sala comum, a página do convite ganhou "Assistir no navegador"; no servidor próprio, o próprio link abre a página. Pelo navegador só dá para assistir; transmitir continua sendo pelo app. Quem assiste pelo navegador aparece com um ícone de globo.
- **Seus servidores na tela inicial**: uma barra de ícones à esquerda, como a do Discord, com os servidores que você administra e os em que já entrou. Passe o mouse para ver quem está lá; clique para entrar; botão direito para copiar o link ou tirar da lista.

### Melhorias
- Para entrar numa sala basta colar o link. Se ele não trouxer a senha, o app pergunta.
- Repasse (TURN) da sala comum: ao criar o token da Cloudflare, dá para escolher com ou sem a permissão do repasse, e um servidor que ficou sem repasse pode ganhar depois, sem montar de novo.
- Quem cai da sala sem avisar (janela recarregada, internet que sumiu) deixa de aparecer duas vezes na lista.
- Recusas da sala dizem o motivo (banido, sala cheia, senha ainda não definida, senhas erradas demais) e o app para de tentar à toa.
- A sala comum também limita as senhas erradas por pessoa (5 por minuto), as pessoas na sala (50) e o tamanho das mensagens.

### Por dentro
- A sala comum continua compatível com quem está na 0.6.0 e anteriores. As salas de servidor próprio e a página do navegador são só da 0.7.0 em diante.

## [0.6.0] - 2026-11-21

### Novidades
- **Servidor de repasse (TURN)**: nova opção em Configurações > Geral. Quando duas pessoas não conseguem se conectar direto (os roteadores não deixam), o vídeo passa por um servidor de repasse, e a tela abre em vez de ficar travada em "conectando".
  - Quem cria a sala configura o próprio servidor: Cloudflare (Key ID e API token), coturn com segredo, ou usuário e senha de qualquer outro serviço.
  - Quem entra na sala não configura nada: o app recebe o servidor junto com a entrada. Só precisa estar na versão 0.6.0.
  - O token e o segredo ficam só no PC de quem criou a sala. Na Cloudflare e no coturn, a sala recebe uma senha temporária.
  - Você escolhe quanto essa senha dura (de 15 minutos a 24 horas, padrão 1 hora). Ela é trocada sozinha antes de vencer, sem derrubar quem está assistindo.
  - Na Cloudflare, a senha antiga é revogada logo depois da troca, e todas quando a sala ou o app fecham. Assim, se alguém tirar a senha da sala, ela para de valer cedo.
  - Se o servidor falhar, a sala abre do mesmo jeito, só com conexão direta, e o log diz o motivo.

### Por dentro
- Compatível com quem ainda está na 0.5.0 e anteriores: todo mundo continua se vendo na mesma sala, só que sem o repasse.

## [0.5.0] - 2026-11-09

### Novidades
- **Envio nativo (experimental)**: nova opção em Configurações > Geral > Desempenho. Sua tela passa a ser codificada uma vez pela placa de vídeo e enviada pelo próprio app a todos que assistem, em vez de uma vez para cada pessoa. Isso deve deixar a transmissão mais leve, principalmente com várias pessoas assistindo.
  - O som vai junto, sincronizado com a imagem.
  - Cada pessoa recebe a qualidade do tamanho em que está vendo, e quem tem internet lenta recebe uma qualidade menor sem piorar a imagem dos outros.
  - Vem desligada e vale a partir da próxima sala em que você entrar. Quem assiste não precisa ligar nada nem atualizar.
  - Se testar, conte como foi e mande o log do dia (o arquivo `abretela-<data>.log` na pasta `logs`, onde o AbreTela está instalado): é o que vai decidir se ela vira o padrão.

### Melhorias
- O log das conexões ficou bem mais detalhado, para descobrir por que alguém não consegue assistir: as placas de rede do PC (avisando VPNs e adaptadores virtuais), as rotas tentadas entre os dois lados, em que etapa a conexão falhou e o motivo mais provável.
- Enquanto você transmite, o log mostra a cada 30 segundos quanto do processador e da placa de vídeo o app está usando.

### Por dentro
- Compatível com quem ainda está na 0.4.0 e na 0.3.0: todo mundo continua se vendo na mesma sala, com ou sem o envio nativo.

## [0.4.0] - 2026-11-04

### Correções
- Quem digita a senha errada agora vê "senha errada" na hora. Antes, às vezes o app ficava 45 segundos tentando entrar e terminava com "não foi possível conectar".
- O app não trava mais ao trocar de tela durante a transmissão quando a nova fonte tem faixas diferentes da anterior (por exemplo, com som para sem som) e alguém está assistindo.

### Por dentro
- O código foi reorganizado em peças separadas (sala, transmissão, captura, túnel), cada uma trocável sem mexer no resto. Isso prepara o envio da tela direto pelo app e as versões para Mac e Linux.
- Testes automáticos novos rodam a cada mudança, cobrindo o convite, a sala, a qualidade por espectador e o formato da captura.
- Compatível com quem ainda está na 0.3.0: as duas versões continuam se vendo na mesma sala.

## [0.3.0] - 2026-11-03

### Novidades
- **Trocar tela sem parar**: quem está ao vivo pode trocar o que está transmitindo sem derrubar quem está assistindo.
- **Prévia de quem está ao vivo**: passe o mouse sobre alguém na lista para ver uma imagem da tela antes de assistir.
- **Tela de abertura**: o app abre com o mascote pulando numa janelinha, como o Discord, e já procura atualizações ali.

### Melhorias
- O nome é pedido só na primeira vez que o app abre. Para mudar, vá em Configurações > Geral > Perfil.

## [0.2.0] - 2026-11-03

### Novidades
- **Convite por link**: o convite agora é um link que abre o AbreTela direto na sala. A página do link mostra quantas pessoas estão na sala e tem o botão de download para quem ainda não tem o app.
- **Janela flutuante**: abra a tela de alguém numa janelinha que fica por cima do jogo.
- **Aceleração de hardware**: nova opção em Configurações para ligar ou desligar o uso da placa de vídeo na transmissão.
- Configurações viraram uma tela própria, com Geral, Aparência e Sobre.
- A sala vazia mostra atalhos para abrir sua tela ou chamar os amigos.

### Melhorias
- Transmitir pesa bem menos no jogo: a captura é convertida na placa de vídeo, a codificação usa H.264 pela placa, e cada pessoa recebe a tela no tamanho em que está vendo (quem está com o app minimizado não recebe vídeo).
- Visual novo, com tons quentes nos temas claro e escuro.
- As opções da sala abrem dentro do próprio card, sem barra de rolagem.
- Logs mais detalhados para achar problemas de conexão e de captura.
- A confirmação para o anfitrião encerrar a sala ganhou a cara do app, e o mascote ficou visível no tema escuro.
- O menu "Inspecionar" do navegador foi bloqueado e entrou o botão "Procurar atualizações".

## [0.1.1] - 2026-10-26

### Melhorias
- A barra lateral de participantes voltou, agora recolhível.

## [0.1.0] - 2026-10-25

Primeira versão.

- Salas com senha, criadas com um clique e acessíveis pela internet sem configurar o roteador.
- Transmissão de apps ou da tela inteira, com o áudio só do app escolhido.
- Várias transmissões ao mesmo tempo, em grade ou com foco em uma.
- Português, inglês, espanhol, chinês, coreano e japonês, com tema claro e escuro.
- Atualização automática, ícone na bandeja, iniciar com o Windows e efeitos sonoros.
