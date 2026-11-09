# Changelog

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
