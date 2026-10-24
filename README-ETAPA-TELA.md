# Voice Screen — Tela MVP 720p / 15 FPS (Windows)

## Alcance

Este patch **experimental** adiciona captura de monitor, envio P2P e visualização no Slint em `720p` (limite 1280×720) a **até 15 FPS**, mantendo voz/volumes e conexão WSS com a VPS. Para facilitar o primeiro teste, usa **JPEG sobre DataChannel WebRTC separado do Opus**, com fragmentação, reagrupamento e descarte de quadros atrasados. **Não é H.264, não é uma trilha de vídeo RTP e não garante 15 FPS sob carga**. A próxima iteração de performance deve migrar para codificação H.264 por hardware e vídeo RTP/RTCP (ou outra solução de mídia apropriada), com backpressure, métricas e ajuste de bitrate. O modelo JPEG é para provar a interface e as conexões de tela.

**Preservar** o código que já funciona: faça backup ou commit antes. A instalação **não altera a VPS**, o Nginx, o Coturn ou `ui-controls.slint`. O arquivo `main.rs` foi baseado **exatamente no arquivo fornecido pelo usuário**; o `p2p.rs` parte da última versão de diagnóstico ICE/Trickle ICE disponível na conversa. Caso tenha alterações locais em `p2p.rs`, compare/mescle com cuidado em vez de sobrescrever cegamente.

## Instalação

Copiar os arquivos para a raiz do projeto, mantendo seus caminhos:

- `apps/desktop/src/main.rs` (interface + botões, Rust)
- `apps/desktop/src/p2p.rs` (DataChannel de tela adicional)
- `apps/desktop/src/screen.rs` (captura e fragmentação)
- `apps/desktop/Cargo.toml` (dependências)

O arquivo `ui-controls.slint` fornecido foi incluído apenas como referência; não há necessidade de substituí-lo.

No Windows/PowerShell, na raiz do workspace:

```powershell
cargo check -p voice-desktop
cargo build --release -p voice-desktop
.\target\release\voice-desktop.exe
```

É necessário Windows com ferramentas MSVC/CMake já usadas para Opus. O XCap pode adicionar dependências nativas ao build.

## Teste em redes diferentes

1. Usar **a mesma versão do aplicativo nos dois PCs**; o antigo não reconhece o DataChannel `voice-screen-jpeg-v1`.
2. Criar uma sala e entrar pelo outro computador. Verificar `Conexao WebRTC: Connected` e a voz normal.
3. Escolher a aba **Tela** e o monitor desejado no PC que apresentará.
4. Clicar em **Compartilhar tela**. No outro PC abrir **Tela** e verificar a imagem recebida.
5. Testar **Parar tela**. A chamada de voz deve continuar funcionando.
6. Repetir com três participantes e anotar CPU, uso de rede, perda de imagens e qualquer atraso na voz. Usar fones para evitar eco.

Se aparecer `Tela atrasada ...`, a codificação/rede está congestionada; os quadros serão descartados para preservar a voz. Se a tela continuar preta, confirmar `Tela P2P disponivel`, conferir a aba **Diagnóstico** e validar se o pacote foi compilado nos dois PCs.

## Limitações conhecidas

- Captura de **monitor inteiro**; ainda não há seleção de janela.
- Um apresentador por vez é **uma convenção da interface**, ainda sem arbitragem de sala no servidor. Não iniciar duas transmissões simultâneas.
- Captura de tela pode incluir janelas privadas e notificações; selecione o monitor correto antes de compartilhar.
- O consumo de CPU e banda com JPEG pode ser maior que com H.264; não considerar este MVP como configuração definitiva para 1440p/60 FPS.
- Se o usuário clicar em **Compartilhar tela** antes de entrar na sala, a captura não inicia. Entre na sala primeiro.
- O Slint exibe apenas um fluxo por vez; ainda não há seleção de qual apresentador assistir.

## Perfis planejados (ainda desabilitados)

Resoluções: 144p (256×144), **560p** (aprox. 996×560), 720p (1280×720), 1080p (1920×1080) e 1440p (2560×1440). Frequências: **15, 30, 60 FPS**. A seleção real desses perfis deve ser habilitada somente após a transição para um codec de vídeo adequado e testes de rede/hardware.

## Validação

Os arquivos foram gerados e inspecionados estaticamente neste ambiente. **Não foram compilados ou testados no Windows aqui**; o primeiro comando a executar é `cargo check`. Em caso de erro, envie o trecho completo.
