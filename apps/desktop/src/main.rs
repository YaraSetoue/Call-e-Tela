mod audio;
mod p2p;
mod screen;
mod screen_audio;

use std::{collections::{HashMap, HashSet}, sync::{Arc, atomic::{AtomicBool, Ordering}}};

use futures_util::{SinkExt, StreamExt};
use p2p::{Mesh, MeshEvent};
use slint::{ComponentHandle, Model};
use tokio::sync::{broadcast, mpsc, watch};
use audio::{AudioSession, PlaybackBus, microphone_names, output_device_names};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use voice_protocol::{ClientMessage, ServerMessage, SignalPayload};

slint::slint! {
    #[style = "fluent"]
    import { ScrollView, Palette } from "std-widgets.slint";

    import { Theme, Button, LineEdit, ComboBox, Slider } from "ui-controls.slint";
    export struct PeerMixRow {
        id: string,
        name: string,
        volume: float,
    }
    export struct LiveStreamRow {
        id: string,
        name: string,
    }

    export component MainWindow inherits Window {
        title: "Voice Screen - Salas de voz";
        preferred-width: 1060px;
        preferred-height: 720px;
        background: Theme.background;
        full-screen: root.fullscreen-video;
        min-width: 900px;
        min-height: 640px;
        default-font-family: "Segoe UI";
        default-font-size: 13px;
        init => { Palette.color-scheme = ColorScheme.dark; }

        // As propriedades e callbacks originais foram mantidos para preservar o Rust.
        in-out property <string> server-url: "wss://voz.virtuabil.cloud/ws";
        in-out property <string> display-name: "Participante";
        in-out property <string> room-code: "";
        in-out property <string> status-text: "Pronto para criar ou entrar em uma sala";
        in-out property <string> participants-text: "Nenhum participante conectado";
        in-out property <string> p2p-text: "O teste P2P iniciara quando um segundo participante entrar.";
        in-out property <bool> microphone-enabled: false;
        in-out property <[string]> microphone-list: ["Padrao do Windows"];
        in-out property <int> microphone-index: 0;
        in-out property <string> microphone-meter: "Microfone: desligado";
        in-out property <[string]> output-list: ["Padrao do Windows"];
        in-out property <int> output-index: 0;
        in-out property <float> master-volume: 100;
        in-out property <[PeerMixRow]> peer-volumes: [];
        in-out property <string> audio-stats-text: "Aguardando participantes para iniciar o canal de voz.";
        in-out property <int> active-section: 0;
        // Compartilhamento inicial: 720p a 15 FPS. Perfis futuros documentados.
        in-out property <bool> screen-sharing: false;
        in-out property <bool> share-system-audio: false;
        in-out property <image> screen-frame;
        in-out property <string> screen-status: "Nenhuma tela sendo compartilhada";
        in-out property <[string]> screen-monitors: ["Monitor principal"];
        in-out property <int> screen-monitor-index: 0;
        in-out property <[LiveStreamRow]> live-streams: [];
        in-out property <string> watching-stream-id: "";
        in-out property <bool> fullscreen-video: false;
        in-out property <int> viewer-count: 0;
        // Perfis aplicados na proxima inicializacao do compartilhamento.
        in-out property <[string]> video-resolutions: ["144p", "560p", "720p", "1080p", "1440p"];
        in-out property <[string]> video-fps-options: ["15 FPS", "30 FPS", "60 FPS"];
        in-out property <int> video-resolution-index: 2;
        in-out property <int> video-fps-index: 0;

        callback create-room();
        callback join-room();
        callback toggle-microphone();
        callback select-microphone(int);
        callback refresh-microphones();
        callback test-speaker();
        callback select-output(int);
        callback refresh-outputs();
        callback change-master-volume(float);
        callback change-peer-volume(string, float);
        callback start-screen();
        callback toggle-screen-audio(bool);
        callback stop-screen();
        callback select-screen-monitor(int);
        callback refresh-screen-monitors();
        callback watch-screen(string);
        callback leave-screen();
        callback select-video-resolution(int);
        callback select-video-fps(int);

        if root.fullscreen-video : VerticalLayout {
            spacing: 0px;
            Rectangle {
                height: 44px;
                background: Theme.background;
                HorizontalLayout {
                    padding-left: 16px;
                    padding-right: 16px;
                    spacing: 12px;
                    Text { text: root.screen-status; color: Theme.text; horizontal-stretch: 1; }
                    Button { text: "Sair da tela cheia"; clicked => { root.fullscreen-video = false; } }
                }
            }
            Rectangle {
                background: #090909;
                vertical-stretch: 1;
                Image {
                    width: parent.width;
                    height: parent.height;
                    source: root.screen-frame;
                    image-fit: contain;
                }
            }
        }

        if !root.fullscreen-video : VerticalLayout {
            spacing: 0px;

            // Cabecalho simples: principal primeiro, configuracoes fora do caminho.
            Rectangle {
                height: 72px;
                background: Theme.background;
                HorizontalLayout {
                    padding-left: 24px;
                    padding-right: 24px;
                    padding-top: 12px;
                    padding-bottom: 12px;
                    spacing: 12px;
                    VerticalLayout {
                        spacing: 2px;
                        Text { text: "Voice Screen"; font-size: 22px; font-weight: 600; color: Theme.text; }
                        Text { text: "Chamadas de voz entre computadores"; font-size: 12px; color: Theme.muted; }
                    }
                    Rectangle { background: transparent; horizontal-stretch: 1; }
                    Button { text: "Chamada"; selected: root.active-section == 0; clicked => { root.active-section = 0; } }
                    Button { text: "Dispositivos"; selected: root.active-section == 1; clicked => { root.active-section = 1; } }
                    Button { text: "Tela"; selected: root.active-section == 3; clicked => { root.active-section = 3; } }
                    Button { text: "Diagnóstico"; selected: root.active-section == 2; clicked => { root.active-section = 2; } }
                }
            }

            // Conteudo: coluna de sala/participantes e painel principal independente.
            HorizontalLayout {
                padding: 16px;
                spacing: 16px;

                Rectangle {
                    width: 290px;
                    background: Theme.surface;
                    border-radius: 8px;
                    border-width: 1px;
                    border-color: Theme.border;
                    VerticalLayout {
                        padding: 18px;
                        spacing: 10px;

                        Text { text: "SUA SALA"; font-size: 13px; font-weight: 700; color: Theme.muted; }
                        Text { text: "Nome"; font-size: 12px; color: Theme.muted; }
                        LineEdit { text <=> root.display-name; }
                        Text { text: "Código de convite"; font-size: 12px; color: Theme.muted; }
                        LineEdit { text <=> root.room-code; }
                        HorizontalLayout {
                            spacing: 8px;
                            Button { text: "Criar sala"; primary: true; clicked => { root.create-room(); } }
                            Button { text: "Entrar"; clicked => { root.join-room(); } }
                        }
                        Rectangle { height: 1px; background: Theme.border; }
                        Text { text: "PARTICIPANTES"; font-size: 13px; font-weight: 700; color: Theme.muted; }
                        Text {
                            text: root.participants-text;
                            font-size: 12px;
                            color: Theme.muted;
                            wrap: word-wrap;
                        }

                        // So a lista rola; acoes de sala permanecem visiveis.
                        ScrollView {
                            VerticalLayout {
                                spacing: 9px;
                                for peer in root.peer-volumes : Rectangle {
                                    height: 80px;
                                    background: Theme.control;
                                    border-radius: 8px;
                                    VerticalLayout {
                                        padding: 10px;
                                        spacing: 3px;
                                        HorizontalLayout {
                                            spacing: 5px;
                                            Text {
                                                text: peer.name;
                                                font-size: 13px;
                                                color: Theme.text;
                                                font-weight: 600;
                                                horizontal-stretch: 1;
                                            }
                                            Text { text: peer.volume + "%"; font-size: 12px; color: Theme.muted; }
                                        }
                                        Slider {
                                            minimum: 0;
                                            maximum: 200;
                                            step: 5;
                                            value: peer.volume;
                                            changed(value) => { root.change-peer-volume(peer.id, value); }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                Rectangle {
                    background: Theme.surface;
                    border-radius: 8px;
                    border-width: 1px;
                    border-color: Theme.border;
                    horizontal-stretch: 1;
                    VerticalLayout {
                        spacing: 0px;

                        // Tela principal: somente o que importa durante a chamada.
                        if root.active-section == 0 : VerticalLayout {
                            padding: 28px;
                            spacing: 18px;
                            Text { text: "Chamada de voz"; font-size: 25px; color: Theme.text; font-weight: 700; }
                            Text { text: "Sua sala e seus controles, em um só lugar."; wrap: word-wrap; color: Theme.muted; font-size: 14px; }
                            Rectangle { height: 1px; background: Theme.border; }
                            VerticalLayout {
                                spacing: 10px;
                                Text { text: "CONEXÃO"; color: Theme.muted; font-size: 12px; font-weight: 600; }
                                Text { text: root.status-text; color: Theme.text; font-size: 18px; wrap: word-wrap; }
                            }
                            Rectangle { height: 1px; background: Theme.border; }
                            VerticalLayout {
                                spacing: 10px;
                                Text { text: "MICROFONE"; font-size: 12px; font-weight: 600; color: Theme.muted; }
                                Text { text: root.microphone-meter; color: Theme.text; font-size: 15px; wrap: word-wrap; }
                            }
                            Rectangle { height: 1px; background: Theme.border; }
                            Text { text: "TRANSMISSOES AO VIVO"; color: Theme.muted; font-size: 12px; font-weight: 600; }
                            if root.live-streams.length == 0 : Text {
                                text: "Ninguem esta compartilhando a tela."; color: Theme.muted; font-size: 12px;
                            }
                            for stream in root.live-streams : HorizontalLayout {
                                spacing: 8px;
                                Text { text: stream.name; color: Theme.text; horizontal-stretch: 1; }
                                Button {
                                    text: root.watching-stream-id == stream.id ? "Assistindo" : "Assistir";
                                    enabled: root.watching-stream-id != stream.id;
                                    clicked => { root.watch-screen(stream.id); }
                                }
                            }
                            if root.watching-stream-id != "" : Button {
                                text: "Sair da transmissao";
                                clicked => { root.leave-screen(); }
                            }
                            Rectangle { background: transparent; vertical-stretch: 1; }
                            Text {
                                text: "Use fones de ouvido para evitar eco durante os testes.";
                                font-size: 12px;
                                color: Theme.muted;
                            }
                        }

                        // Configuracoes agrupadas: nao ocupam espaco na chamada.
                        if root.active-section == 1 : ScrollView {
                            VerticalLayout {
                                padding: 28px;
                                spacing: 14px;
                                Text { text: "Dispositivos e áudio"; font-size: 22px; font-weight: 600; color: Theme.text; }
                                Text { text: "Escolha o microfone e onde deseja ouvir os participantes."; wrap: word-wrap; font-size: 14px; color: Theme.muted; }
                                Rectangle { height: 1px; background: Theme.border; }
                                Text { text: "Microfone de entrada"; font-size: 14px; font-weight: 700; color: Theme.text; }
                                HorizontalLayout {
                                    spacing: 10px;
                                    mic_selector := ComboBox {
                                        model: root.microphone-list;
                                        current-index: root.microphone-index;
                                        selected(_value) => { root.select-microphone(mic_selector.current-index); }
                                    }
                                    Button { text: "Atualizar"; clicked => { root.refresh-microphones(); } }
                                }
                                Text { text: "Dispositivo de saída"; font-size: 14px; font-weight: 700; color: Theme.text; }
                                HorizontalLayout {
                                    spacing: 10px;
                                    output_selector := ComboBox {
                                        model: root.output-list;
                                        current-index: root.output-index;
                                        selected(_value) => { root.select-output(output_selector.current-index); }
                                    }
                                    Button { text: "Atualizar"; clicked => { root.refresh-outputs(); } }
                                }
                                Rectangle { height: 1px; background: Theme.border; }
                                Text { text: "Volume geral de saída"; font-size: 14px; font-weight: 700; color: Theme.text; }
                                HorizontalLayout {
                                    spacing: 10px;
                                    Slider {
                                        minimum: 0;
                                        maximum: 200;
                                        step: 5;
                                        value <=> root.master-volume;
                                        changed(value) => { root.change-master-volume(value); }
                                    }
                                    Text { text: root.master-volume + "%"; width: 70px; color: Theme.muted; }
                                }
                                Text { text: "O volume de cada pessoa pode ser ajustado na coluna Participantes."; font-size: 12px; color: Theme.muted; wrap: word-wrap; }
                                Rectangle { background: transparent; vertical-stretch: 1; }
                            }
                        }

                         // Visualizacao de tela. Botao de compartilhar tambem no rodape.
                         if root.active-section == 3 : VerticalLayout {
                             padding: 20px;
                             spacing: 10px;
                             Text { text: "Compartilhamento de tela"; font-size: 22px; font-weight: 600; color: Theme.text; }
                             Text { text: "H.264 / Opus - Qualidade configuravel por quem compartilha"; color: Theme.muted; font-size: 13px; }
                             Text { text: "TRANSMISSOES DISPONIVEIS"; color: Theme.muted; font-size: 12px; font-weight: 600; }
                             if root.live-streams.length == 0 : Text {
                                 text: "Nenhuma transmissao ativa no momento.";
                                 color: Theme.muted; font-size: 12px;
                             }
                             for stream in root.live-streams : HorizontalLayout {
                                 spacing: 10px;
                                 Text { text: stream.name + " esta ao vivo"; color: Theme.text; horizontal-stretch: 1; }
                                 Button {
                                     text: root.watching-stream-id == stream.id ? "Assistindo" : "Assistir transmissao";
                                     enabled: root.watching-stream-id != stream.id;
                                     clicked => { root.watch-screen(stream.id); }
                                 }
                             }
                             if root.watching-stream-id != "" : Button {
                                 text: "Sair da transmissao (continuar na chamada)";
                                 clicked => { root.leave-screen(); }
                             }
                             Rectangle { height: 1px; background: Theme.border; }
                             HorizontalLayout {
                                 Text { text: "MINHA TRANSMISSAO"; color: Theme.muted; font-size: 12px; font-weight: 600; horizontal-stretch: 1; }
                                 Text { text: root.screen-sharing ? "Espectadores: " + root.viewer-count : "Transmissao inativa";
                                        color: Theme.text; font-size: 13px; }
                             }
                             HorizontalLayout {
                                 spacing: 10px;
                                 Text { text: "Qualidade"; color: Theme.muted; font-size: 12px; }
                                 resolution_picker := ComboBox {
                                     model: root.video-resolutions;
                                     current-index: root.video-resolution-index;
                                     selected(_value) => { root.select-video-resolution(resolution_picker.current-index); }
                                 }
                                 fps_picker := ComboBox {
                                     model: root.video-fps-options;
                                     current-index: root.video-fps-index;
                                     selected(_value) => { root.select-video-fps(fps_picker.current-index); }
                                 }
                             }
                             Text { text: "Novas opcoes de qualidade valem no proximo compartilhamento; 1080p/1440p e 30/60 FPS exigem mais CPU.";
                                    font-size: 12px; color: Theme.muted; wrap: word-wrap; }
                             HorizontalLayout {
                                 spacing: 10px;
                                 monitor_picker := ComboBox {
                                     model: root.screen-monitors;
                                     current-index: root.screen-monitor-index;
                                     selected(_value) => { root.select-screen-monitor(monitor_picker.current-index); }
                                 }
                                 Button { text: "Atualizar"; clicked => { root.refresh-screen-monitors(); } }
                             }
                             Button {
                                 text: root.share-system-audio ? "Som do computador: LIGADO" : "Som do computador: desligado";
                                 clicked => {
                                     root.share-system-audio = !root.share-system-audio;
                                     root.toggle-screen-audio(root.share-system-audio);
                                 }
                             }
                             Text { text: "Opcional: transmite videos e apps do Windows, sem repetir a voz da chamada. Requer Windows 11 ou Windows 10 atualizado."; color: Theme.muted; font-size: 12px; wrap: word-wrap; }
                             Text { text: root.screen-status; wrap: word-wrap; color: Theme.muted; font-size: 12px; }
                             if root.watching-stream-id != "" || root.screen-sharing : Button {
                                 text: "Tela cheia";
                                 clicked => { root.fullscreen-video = true; }
                             }
                             Rectangle {
                                 background: #090909;
                                 border-width: 1px;
                                 border-color: Theme.border;
                                 border-radius: 6px;
                                 vertical-stretch: 1;
                                 Image {
                                     width: parent.width;
                                     height: parent.height;
                                     source: root.screen-frame;
                                     image-fit: contain;
                                 }
                             }
                             Text { text: "Entrar/sair da transmissao nao desconecta voce da voz. Nesta versao, assista a uma tela por vez.";
                                    font-size: 12px; color: Theme.muted; wrap: word-wrap; }
                         }

                        // Logs ficam numa aba separada, sem poluir a experiencia.
                        if root.active-section == 2 : ScrollView {
                            VerticalLayout {
                                padding: 28px;
                                spacing: 12px;
                                Text { text: "Diagnóstico da chamada"; font-size: 22px; font-weight: 600; color: Theme.text; }
                                Text { text: "Se 'enviados' aumenta mas 'recebidos' e zero no outro PC, verificar canal P2P. Se 'recebidos' aumenta sem som, verificar saida e volumes. Use fones para evitar eco."; font-size: 13px; color: Theme.muted; wrap: word-wrap; }
                                Rectangle { height: 1px; background: Theme.border; }
                                Text { text: "Pacotes Opus por participante"; font-size: 14px; font-weight: 700; color: Theme.text; }
                                Text { text: root.audio-stats-text; font-size: 13px; color: Theme.text; wrap: word-wrap; }
                                Rectangle { height: 1px; background: Theme.border; }
                                Text { text: "Eventos WebRTC"; font-size: 14px; font-weight: 700; color: Theme.text; }
                                Text { text: root.p2p-text; font-size: 12px; color: Theme.muted; wrap: word-wrap; }
                                Rectangle { height: 1px; background: Theme.border; }
                                Text { text: "Servidor de sinalização"; font-size: 14px; font-weight: 700; color: Theme.text; }
                                LineEdit { text <=> root.server-url; }
                                Text { text: "Alterações na URL passam a valer ao criar/entrar na próxima sala."; font-size: 12px; color: Theme.muted; wrap: word-wrap; }
                                Rectangle { background: transparent; vertical-stretch: 1; }
                            }
                        }
                    }
                }
            }

            // A barra de chamada fica sempre visivel, sem rolagem.
            Rectangle {
                height: 72px;
                background: Theme.background;
                HorizontalLayout {
                    padding-left: 24px;
                    padding-right: 24px;
                    padding-top: 14px;
                    padding-bottom: 14px;
                    spacing: 14px;
                    Text { text: root.microphone-enabled ? "Microfone ligado" : "Microfone desligado"; color: Theme.muted; font-size: 14px; }
                    Rectangle { background: transparent; horizontal-stretch: 1; }
                    Button { text: "Testar saída"; clicked => { root.test-speaker(); } }
                    Button {
                         text: root.screen-sharing ? "Parar tela" : "Compartilhar tela";
                         clicked => {
                             if root.screen-sharing { root.stop-screen(); }
                             else { root.active-section = 3; root.start-screen(); }
                         }
                     }
                    Button {
                        primary: true;
                        text: root.microphone-enabled ? "Desligar microfone" : "Ligar microfone";
                        clicked => { root.toggle-microphone(); }
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
struct ConnectionCommand {
    url: String,
    message: ClientMessage,
}

fn update_ui(
    weak: &slint::Weak<MainWindow>,
    room: Option<String>,
    status: String,
    users: String,
    p2p_log: String,
) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        if let Some(room) = room { ui.set_room_code(room.into()); }
        ui.set_status_text(status.into());
        ui.set_participants_text(users.into());
        ui.set_p2p_text(p2p_log.into());
    });
}

fn user_list(peers: &HashMap<String, String>) -> String {
    let mut names: Vec<&str> = peers.values().map(String::as_str).collect();
    names.sort_unstable();
    if names.is_empty() { "Voce e o primeiro participante.".into() }
    else { format!("Outros participantes: {}", names.join(", ")) }
}

fn update_peer_volumes(
    weak: &slint::Weak<MainWindow>,
    peers: &HashMap<String, String>,
    playback: &PlaybackBus,
) {
    let mut rows = peers.iter().map(|(id, name)| PeerMixRow {
        id: id.clone().into(),
        name: name.clone().into(),
        volume: playback.peer_percent(id),
    }).collect::<Vec<_>>();
    rows.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.set_peer_volumes(slint::ModelRc::new(slint::VecModel::from(rows)));
    });
}

fn update_live_streams(
    weak: &slint::Weak<MainWindow>,
    peers: &HashMap<String, String>,
    live: &std::collections::HashSet<String>,
) {
    let mut rows: Vec<LiveStreamRow> = live.iter().filter_map(|id| {
        peers.get(id).map(|name| LiveStreamRow {
            id: id.clone().into(), name: name.clone().into(),
        })
    }).collect();
    rows.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.set_live_streams(slint::ModelRc::new(slint::VecModel::from(rows)));
    });
}

fn update_voice_stats(
    weak: &slint::Weak<MainWindow>,
    peers: &HashMap<String, String>,
    stats: &HashMap<String, (u64, u64)>,
) {
    let mut lines: Vec<_> = peers.iter().map(|(id, name)| {
        let (sent, received) = stats.get(id).copied().unwrap_or((0, 0));
        format!("{name}: enviados {sent}, recebidos {received}")
    }).collect();
    lines.sort();
    let summary = if lines.is_empty() {
        "Nenhum participante remoto conectado.".to_owned()
    } else { lines.join("\n") };
    let _ = weak.upgrade_in_event_loop(move |ui| ui.set_audio_stats_text(summary.into()));
}

fn add_log(log: &mut Vec<String>, message: String) {
    log.push(message);
    if log.len() > 35 { log.remove(0); }
}

async fn connection(
    weak: slint::Weak<MainWindow>,
    command: ConnectionCommand,
    mut mic_state: watch::Receiver<bool>,
    mut selected_mic: watch::Receiver<usize>,
    mut selected_output: watch::Receiver<usize>,
    mut speaker_test: watch::Receiver<u64>,
     mut screen_state: watch::Receiver<Option<usize>>,
    quality: watch::Receiver<screen::VideoProfile>,
    mut system_audio_state: watch::Receiver<bool>,
    mut watch_state: watch::Receiver<Option<String>>,
    watch_tx: watch::Sender<Option<String>>,
    mut cancel: watch::Receiver<bool>,
    playback: PlaybackBus,
) {
    watch_tx.send_replace(None);
    let _ = weak.upgrade_in_event_loop(|ui| {
        ui.set_watching_stream_id("".into());
        ui.set_live_streams(slint::ModelRc::new(slint::VecModel::from(Vec::<LiveStreamRow>::new())));
    });
    update_ui(&weak, None, "Conectando ao servidor...".into(), "".into(), "".into());
    let (socket, _) = match connect_async(&command.url).await {
        Ok(pair) => pair,
        Err(e) => {
            update_ui(&weak, None, format!("Falha de conexao: {e}"), "".into(), "".into());
            return;
        }
    };
    let (mut sink, mut stream) = socket.split();
    let json = match serde_json::to_string(&command.message) {
        Ok(v) => v,
        Err(e) => {
            update_ui(&weak, None, format!("Erro serializando mensagem: {e}"), "".into(), "".into());
            return;
        }
    };
    if let Err(e) = sink.send(Message::Text(json.into())).await {
        update_ui(&weak, None, format!("Falha ao enviar: {e}"), "".into(), "".into());
        return;
    }

    let (signal_tx, mut signal_rx) = mpsc::unbounded_channel::<ClientMessage>();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<MeshEvent>();
     let (screen_frames_tx, _) = broadcast::channel::<Vec<u8>>(2);
     let (system_audio_tx, _) = broadcast::channel::<Vec<u8>>(64);
    let audio = AudioSession::new(event_tx.clone(), playback);
    let mesh = Arc::new(Mesh::new(
        signal_tx, event_tx.clone(), audio.frames.clone(), screen_frames_tx.clone(), system_audio_tx.clone(), audio.playback.clone(),
    ));
    audio.output(*selected_output.borrow());
    audio.capture(*mic_state.borrow(), *selected_mic.borrow());
    let mut peers: HashMap<String, String> = HashMap::new();
    let mut live_streams: HashSet<String> = HashSet::new();
    let mut viewers: HashSet<String> = HashSet::new();
    let mut voice_stats: HashMap<String, (u64, u64)> = HashMap::new();
    let mut log: Vec<String> = Vec::new();
     let mut screen_capture: Option<screen::CaptureHandle> = None;
     let mut system_audio_capture: Option<screen_audio::CaptureHandle> = None;
     let mut screen_owner: Option<String> = None;
     let decode_busy = Arc::new(AtomicBool::new(false));
    let mut status = "Conectado ao WebSocket".to_owned();
    let mut room = None;

    loop {
        tokio::select! {
            _ = cancel.changed() => { break; }
            changed = mic_state.changed() => {
                if changed.is_err() { break; }
                audio.capture(*mic_state.borrow(), *selected_mic.borrow());
            }
            changed = selected_mic.changed() => {
                if changed.is_err() { break; }
                if *mic_state.borrow() { audio.capture(true, *selected_mic.borrow()); }
            }
            changed = selected_output.changed() => {
                if changed.is_err() { break; }
                audio.output(*selected_output.borrow());
            }
            changed = watch_state.changed() => {
                if changed.is_err() { break; }
                let target = watch_state.borrow().clone();
                mesh.set_view_target(target.clone());
                screen_owner = target.clone();
                decode_busy.store(false, Ordering::Release);
                let label = target.as_ref().and_then(|id| peers.get(id)).cloned();
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.set_screen_frame(slint::Image::default());
                    ui.set_watching_stream_id(target.clone().unwrap_or_default().into());
                    if target.is_none() { ui.set_fullscreen_video(false); }
                    if let Some(label) = label {
                        ui.set_screen_status(format!("Entrando na transmissao de {label}...").into());
                    } else {
                        ui.set_screen_status("Voce saiu da transmissao. A voz continua ativa.".into());
                    }
                });
                // O fluxo Opus do microfone e a chamada P2P permanecem ativos.
            }
             changed = screen_state.changed() => {
                 if changed.is_err() { break; }
                 // Interrompe captura anterior antes de iniciar outro monitor.
                 mesh.set_local_screen_live(false);
                 drop(system_audio_capture.take());
                 drop(screen_capture.take());
                 let _ = screen_frames_tx.send(b"VSS0".to_vec());
                 viewers.clear();
                 let _ = weak.upgrade_in_event_loop(|ui| ui.set_viewer_count(0));
                 let _ = event_tx.send(MeshEvent::ScreenStopped { peer: "__local__".to_owned() });
                 match *screen_state.borrow() {
                     Some(index) if room.is_some() => {
                         let profile = *quality.borrow();
                         match screen::CaptureHandle::start(index, screen_frames_tx.clone(), event_tx.clone(), profile) {
                             Ok(capture) => {
                                 screen_capture = Some(capture);
                                 mesh.set_local_screen_live(true);
                                 add_log(&mut log, format!("Compartilhando tela: {}", profile.label()));
                                 if *system_audio_state.borrow() {
                                     match screen_audio::CaptureHandle::start(system_audio_tx.clone(), event_tx.clone()) {
                                         Ok(handle) => { system_audio_capture = Some(handle); }
                                         Err(err) => add_log(&mut log, format!("Som da tela indisponivel: {err}")),
                                     }
                                 }
                             }
                             Err(err) => {
                                 add_log(&mut log, format!("Nao foi possivel compartilhar tela: {err}"));
                             }
                         }
                     }
                     Some(_) => add_log(&mut log, "Entre em uma sala antes de compartilhar tela".into()),
                     None => add_log(&mut log, "Compartilhamento encerrado".into()),
                 }
                 update_ui(&weak, room.clone(), status.clone(), user_list(&peers), log.join("\n"));
             }
            changed = system_audio_state.changed() => {
                if changed.is_err() { break; }
                // O som do sistema e independente do microfone e da captura JPEG.
                drop(system_audio_capture.take());
                if *system_audio_state.borrow() && screen_capture.is_some() {
                    match screen_audio::CaptureHandle::start(system_audio_tx.clone(), event_tx.clone()) {
                        Ok(handle) => { system_audio_capture = Some(handle); }
                        Err(err) => add_log(&mut log, format!("Som da tela indisponivel: {err}")),
                    }
                } else {
                    add_log(&mut log, "Transmissao do som do computador desligada".to_owned());
                }
                update_ui(&weak, room.clone(), status.clone(), user_list(&peers), log.join("\n"));
            }
            changed = speaker_test.changed() => {
                if changed.is_err() { break; }
                audio.playback.test_speaker();
                add_log(&mut log, "Tom de teste enviado ao dispositivo de saida".into());
                update_ui(&weak, room.clone(), status.clone(), user_list(&peers), log.join("\n"));
            }
            incoming = stream.next() => {
                let Some(msg) = incoming else { break; };
                let text = match msg {
                    Ok(Message::Text(s)) => s,
                    Ok(Message::Close(_)) => break,
                    Ok(_) => continue,
                    Err(e) => {
                        status = format!("Conexao WSS interrompida: {e}");
                        break;
                    }
                };
                let Ok(message) = serde_json::from_str::<ServerMessage>(&text) else { continue; };
                match message {
                    ServerMessage::RoomCreated { room_id, .. } => {
                        add_log(&mut log, "Sala criada; aguardando outros participantes".into());
                        room = Some(room_id);
                        status = "Sala criada. Aguarde novos participantes.".into();
                    }
                    ServerMessage::RoomJoined { room_id, peers: members, .. } => {
                        add_log(&mut log, format!("Entrei na sala; vou iniciar ofertas para {} participantes", members.len()));
                        room = Some(room_id);
                        status = "Voce entrou na sala. Iniciando conexoes P2P...".into();
                        for peer in members {
                            peers.insert(peer.id.clone(), peer.name);
                            let mesh = mesh.clone();
                            let event_tx = event_tx.clone();
                            tokio::spawn(async move {
                                if let Err(e) = mesh.offer(&peer.id).await {
                                    let _ = event_tx.send(MeshEvent::Status(format!(
                                        "Falha ao negociar com {}: {e}", peer.id
                                    )));
                                }
                            });
                        }
                    }
                    ServerMessage::PeerJoined { peer } => {
                        add_log(&mut log, format!("Participante {} entrou; aguardando oferta SDP", peer.name));
                        status = format!("{} entrou na sala. Aguardando oferta WebRTC...", peer.name);
                        peers.insert(peer.id, peer.name);
                    }
                    ServerMessage::TurnCredentials { urls, username, credential, expires_at: _ } => {
                        // Nao registrar a senha TURN em logs nem na UI.
                        mesh.set_turn_credentials(urls, username, credential).await;
                    }
                    ServerMessage::PeerLeft { peer_id } => {
                        let name = peers.remove(&peer_id).unwrap_or_else(|| "Participante".into());
                        voice_stats.remove(&peer_id);
                        live_streams.remove(&peer_id);
                        if viewers.remove(&peer_id) {
                            let count = viewers.len() as i32;
                            let _ = weak.upgrade_in_event_loop(move |ui| ui.set_viewer_count(count));
                        }
                        update_live_streams(&weak, &peers, &live_streams);
                        let was_watching = watch_state.borrow().as_deref() == Some(peer_id.as_str());
                        if was_watching { watch_tx.send_replace(None); }
                        status = format!("{name} saiu da sala.");
                        mesh.leave(&peer_id).await;
                    }
                    ServerMessage::Signal { from, payload } => {
                        match payload {
                            SignalPayload::Offer { sdp } => {
                                add_log(&mut log, format!("Recebi oferta SDP de {}", &from[..from.len().min(8)]));
                                let mesh = mesh.clone();
                                let event_tx = event_tx.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = mesh.answer(&from, sdp).await {
                                        let _ = event_tx.send(MeshEvent::Status(format!(
                                            "Falha na resposta WebRTC de {from}: {e}"
                                        )));
                                    }
                                });
                            }
                            SignalPayload::Answer { sdp } => {
                                add_log(&mut log, format!("Recebi resposta SDP de {}", &from[..from.len().min(8)]));
                                if let Err(e) = mesh.apply_answer(&from, sdp).await {
                                    add_log(&mut log, format!("Resposta WebRTC invalida: {e}"));
                                }
                            }
                            SignalPayload::IceCandidate {
                                candidate,
                                sdp_mid,
                                sdp_mline_index,
                            } => {
                                add_log(
                                    &mut log,
                                    format!("ICE remoto RECEBIDO de {}", &from[..from.len().min(8)]),
                                );

                                match mesh
                                    .apply_candidate(&from, candidate, sdp_mid, sdp_mline_index)
                                    .await
                                {
                                    Ok(_) => {
                                        add_log(
                                            &mut log,
                                            format!("ICE remoto APLICADO de {}", &from[..from.len().min(8)]),
                                        );
                                    }
                                    Err(e) => {
                                        add_log(
                                            &mut log,
                                            format!("ICE remoto REJEITADO: {e}"),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    ServerMessage::Error { code, message } => {
                        status = format!("Erro {code}: {message}");
                    }
                }
                update_ui(&weak, room.clone(), status.clone(), user_list(&peers), log.join("\n"));
                update_peer_volumes(&weak, &peers, &audio.playback);
                update_voice_stats(&weak, &peers, &voice_stats);
            }
            Some(outgoing) = signal_rx.recv() => {
                match serde_json::to_string(&outgoing) {
                    Ok(json) => {
                        if let Err(e) = sink.send(Message::Text(json.into())).await {
                            status = format!("Falha ao enviar sinalizacao: {e}");
                            break;
                        }
                    }
                    Err(e) => add_log(&mut log, format!("Erro ao serializar SDP: {e}")),
                }
            }
            Some(event) = event_rx.recv() => {
                match event {
                    MeshEvent::Status(message) => add_log(&mut log, message),
                    MeshEvent::ViewerChanged { peer, watching } => {
                        if watching { viewers.insert(peer); }
                        else { viewers.remove(&peer); }
                        let count = if screen_capture.is_some() { viewers.len() as i32 } else { 0 };
                        let _ = weak.upgrade_in_event_loop(move |ui| ui.set_viewer_count(count));
                        continue;
                    },
                    MeshEvent::VoiceStats { peer, sent, received } => {
                        voice_stats.insert(peer, (sent, received));
                        update_voice_stats(&weak, &peers, &voice_stats);
                    }
                     MeshEvent::StreamAvailability { peer, live } => {
                         if live { live_streams.insert(peer.clone()); }
                         else { live_streams.remove(&peer); }
                         update_live_streams(&weak, &peers, &live_streams);
                         let was_watching = watch_state.borrow().as_deref() == Some(peer.as_str());
                         if !live && was_watching { watch_tx.send_replace(None); }
                         continue;
                     }
                     MeshEvent::ScreenFrame { peer, jpeg } => {
                         // Um compartilhamento por vez na interface nesta etapa.
                         let allowed = if peer == "__local__" {
                             watch_state.borrow().is_none() && screen_capture.is_some()
                         } else {
                             watch_state.borrow().as_deref() == Some(peer.as_str())
                         };
                         if allowed { screen_owner = Some(peer.clone()); }
                         if allowed && !decode_busy.swap(true, Ordering::AcqRel)
                         {
                             let weak_image = weak.clone();
                             let busy = decode_busy.clone();
                             tokio::task::spawn_blocking(move || {
                                 if let Ok(image) = image::load_from_memory(&jpeg) {
                                     let rgb = image.to_rgb8();
                                     let pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::clone_from_slice(
                                         rgb.as_raw(), rgb.width(), rgb.height()
                                     );
                                     let owner_label = if peer == "__local__" { "Sua tela".to_string() }
                                         else { format!("Tela de {}", &peer[..peer.len().min(8)]) };
                                     let resolution = format!("{}x{}", rgb.width(), rgb.height());
                                     let _ = weak_image.upgrade_in_event_loop(move |ui| {
                                         ui.set_screen_frame(slint::Image::from_rgb8(pixels));
                                         ui.set_screen_status(format!("{owner_label} - {resolution}").into());
                                     });
                                 }
                                 busy.store(false, Ordering::Release);
                             });
                         }
                         continue;
                     }
                     MeshEvent::ScreenRgbFrame { peer, width, height, rgb } => {
                         let allowed = if peer == "__local__" {
                             watch_state.borrow().is_none() && screen_capture.is_some()
                         } else {
                             watch_state.borrow().as_deref() == Some(peer.as_str())
                         };
                         if allowed { screen_owner = Some(peer.clone()); }
                         if allowed && rgb.len() == width as usize * height as usize * 3
                             && !decode_busy.swap(true, Ordering::AcqRel)
                         {
                             let pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::clone_from_slice(
                                 &rgb, width, height
                             );
                             let owner_label = if peer == "__local__" {
                                 "Sua tela".to_owned()
                             } else {
                                 format!("Tela de {}", &peer[..peer.len().min(8)])
                             };
                             let busy = decode_busy.clone();
                             let _ = weak.upgrade_in_event_loop(move |ui| {
                                 ui.set_screen_frame(slint::Image::from_rgb8(pixels));
                                 ui.set_screen_status(format!(
                                     "{owner_label} - {width}x{height}"
                                 ).into());
                                 busy.store(false, Ordering::Release);
                             });
                         }
                         continue;
                     }
                     MeshEvent::ScreenStopped { peer } => {
                         if screen_owner.as_deref() == Some(peer.as_str()) {
                             screen_owner = None;
                             let weak_screen = weak.clone();
                             let _ = weak_screen.upgrade_in_event_loop(move |ui| {
                                 ui.set_screen_frame(slint::Image::default());
                                 ui.set_screen_status("Compartilhamento encerrado".into());
                             });
                         }
                         continue;
                     }
                    MeshEvent::MicLevel(level) => {
                        let mic_label = if level < 0.005 {
                            "Microfone: sinal muito baixo / silencio".to_owned()
                        } else {
                            format!("Microfone: sinal capturado (pico {:.1}%)", level * 100.0)
                        };
                        let weak_for_meter = weak.clone();
                        let _ = weak_for_meter.upgrade_in_event_loop(move |ui| {
                            ui.set_microphone_meter(mic_label.into());
                        });
                    }
                }
                update_ui(&weak, room.clone(), status.clone(), user_list(&peers), log.join("\n"));
            }
        }
    }
     mesh.set_local_screen_live(false);
     mesh.set_view_target(None);
     watch_tx.send_replace(None);
     drop(system_audio_capture.take());
     drop(screen_capture.take());
     let _ = screen_frames_tx.send(b"VSS0".to_vec());
    audio.capture(false, *selected_mic.borrow());
    mesh.close_all().await;
    update_ui(&weak, None, format!("Desconectado. {status}"), "".into(), log.join("\n"));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Necessario quando dependencias habilitam simultaneamente ring e aws-lc-rs.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Falha ao configurar o CryptoProvider TLS");

    let ui = MainWindow::new()?;
    let (tx, mut rx) = mpsc::unbounded_channel::<ConnectionCommand>();
    let (mic_tx, mic_rx) = watch::channel(false);
    let (selected_tx, selected_rx) = watch::channel(0usize);
    let (test_tx, test_rx) = watch::channel(0u64);
    let (output_tx, output_rx) = watch::channel(0usize);
     let (screen_tx, screen_rx) = watch::channel(None::<usize>);
    let (quality_tx, quality_rx) = watch::channel(screen::VideoProfile::from_indices(2, 0));
     let (sysaudio_tx, sysaudio_rx) = watch::channel(false);
     let (watch_tx, watch_rx) = watch::channel(None::<String>);
    let playback = PlaybackBus::default();
    let microphones = microphone_names();
    let microphone_model = slint::VecModel::from(microphones.into_iter()
        .map(slint::SharedString::from).collect::<Vec<_>>());
    ui.set_microphone_list(slint::ModelRc::new(microphone_model));
    let outputs = output_device_names();
    let output_model = slint::VecModel::from(outputs.into_iter()
        .map(slint::SharedString::from).collect::<Vec<_>>());
    ui.set_output_list(slint::ModelRc::new(output_model));
     ui.set_screen_monitors(slint::ModelRc::new(slint::VecModel::from(
         screen::monitor_names().into_iter().map(slint::SharedString::from).collect::<Vec<_>>()
     )));
    let weak = ui.as_weak();
    let session_playback = playback.clone();
    let watch_tx_for_runtime = watch_tx.clone();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all().build().expect("Erro criando runtime Tokio");
        runtime.block_on(async move {
            let mut job: Option<(watch::Sender<bool>, tokio::task::JoinHandle<()>)> = None;
            while let Some(command) = rx.recv().await {
                if let Some((stop, old)) = job.take() {
                    stop.send_replace(true);
                    let _ = old.await;
                }
                let (stop, cancel) = watch::channel(false);
                let mic = mic_rx.clone();
                let selected = selected_rx.clone();
                let test = test_rx.clone();
                let output = output_rx.clone();
                 let screen_state = screen_rx.clone();
                 let quality = quality_rx.clone();
                 let system_audio_state = sysaudio_rx.clone();
                 let watch_state = watch_rx.clone();
                 let watching_sender = watch_tx_for_runtime.clone();
                job = Some((stop, tokio::spawn(connection(
                     weak.clone(), command, mic, selected, output, test, screen_state, quality, system_audio_state,
                     watch_state, watching_sender, cancel, session_playback.clone(),
                ))));
            }
            if let Some((stop, task)) = job {
                stop.send_replace(true);
                let _ = task.await;
            }
        });
    });
    let sender = tx.clone();
    let weak_for_create = ui.as_weak();
    ui.on_create_room(move || {
        if let Some(ui) = weak_for_create.upgrade() {
            let command = ConnectionCommand {
                url: ui.get_server_url().to_string(),
                message: ClientMessage::CreateRoom { name: ui.get_display_name().to_string() },
            };
            let _ = sender.send(command);
        }
    });
    let weak_for_join = ui.as_weak();
    ui.on_join_room(move || {
        if let Some(ui) = weak_for_join.upgrade() {
            let command = ConnectionCommand {
                url: ui.get_server_url().to_string(),
                message: ClientMessage::JoinRoom {
                    name: ui.get_display_name().to_string(),
                    room_id: ui.get_room_code().to_string(),
                },
            };
            let _ = tx.send(command);
        }
    });
    let mic_toggle = mic_tx;
    let weak_for_microphone = ui.as_weak();
    ui.on_toggle_microphone(move || {
        if let Some(ui) = weak_for_microphone.upgrade() {
            let enabled = !ui.get_microphone_enabled();
            ui.set_microphone_enabled(enabled);
            mic_toggle.send_replace(enabled);
        }
    });
    let select_tx = selected_tx.clone();
    let weak_for_select = ui.as_weak();
    ui.on_select_microphone(move |index| {
        if index >= 0 {
            select_tx.send_replace(index as usize);
            if let Some(ui) = weak_for_select.upgrade() {
                ui.set_microphone_index(index);
                ui.set_microphone_meter("Microfone selecionado (ative para testar)".into());
            }
        }
    });
    let weak_for_refresh = ui.as_weak();
    let refresh_tx = selected_tx.clone();
    ui.on_refresh_microphones(move || {
        if let Some(ui) = weak_for_refresh.upgrade() {
            let names = microphone_names();
            ui.set_microphone_index(0);
            refresh_tx.send_replace(0);
            let model = slint::VecModel::from(names.into_iter()
                .map(slint::SharedString::from).collect::<Vec<_>>());
            ui.set_microphone_list(slint::ModelRc::new(model));
            ui.set_microphone_meter("Lista de microfones atualizada".into());
        }
    });
    let change_output_tx = output_tx.clone();
    let weak_for_output = ui.as_weak();
    ui.on_select_output(move |index| {
        if index >= 0 {
            change_output_tx.send_replace(index as usize);
            if let Some(ui) = weak_for_output.upgrade() { ui.set_output_index(index); }
        }
    });
    let refresh_output_tx = output_tx.clone();
    let weak_for_output_refresh = ui.as_weak();
    ui.on_refresh_outputs(move || {
        if let Some(ui) = weak_for_output_refresh.upgrade() {
            ui.set_output_index(0);
            refresh_output_tx.send_replace(0);
            let names = output_device_names();
            let model = slint::VecModel::from(names.into_iter()
                .map(slint::SharedString::from).collect::<Vec<_>>());
            ui.set_output_list(slint::ModelRc::new(model));
        }
    });
    let master_bus = playback.clone();
    ui.on_change_master_volume(move |percent| {
        master_bus.set_master_percent(percent);
    });
    let peer_bus = playback.clone();
    let weak_for_peer_gain = ui.as_weak();
    ui.on_change_peer_volume(move |peer_id, percent| {
        peer_bus.set_peer_percent(peer_id.as_str(), percent);
        if let Some(ui) = weak_for_peer_gain.upgrade() {
            let model = ui.get_peer_volumes();
            if let Some(rows) = model.as_any().downcast_ref::<slint::VecModel<PeerMixRow>>() {
                for index in 0..rows.row_count() {
                    if let Some(mut row) = rows.row_data(index) {
                        if row.id == peer_id {
                            row.volume = percent;
                            rows.set_row_data(index, row);
                            break;
                        }
                    }
                }
            }
        }
    });
    ui.on_test_speaker(move || {
        let next = (*test_tx.borrow()).wrapping_add(1);
        test_tx.send_replace(next);
    });
     let resolutions_tx = quality_tx.clone();
     let weak_resolution = ui.as_weak();
     ui.on_select_video_resolution(move |index| {
         if let Some(ui) = weak_resolution.upgrade() {
             let value = index.clamp(0, 4);
             ui.set_video_resolution_index(value);
             resolutions_tx.send_replace(screen::VideoProfile::from_indices(
                 value as usize, ui.get_video_fps_index().max(0) as usize,
             ));
         }
     });
     let fps_tx = quality_tx;
     let weak_fps = ui.as_weak();
     ui.on_select_video_fps(move |index| {
         if let Some(ui) = weak_fps.upgrade() {
             let value = index.clamp(0, 2);
             ui.set_video_fps_index(value);
             fps_tx.send_replace(screen::VideoProfile::from_indices(
                 ui.get_video_resolution_index().max(0) as usize, value as usize,
             ));
         }
     });
     ui.on_toggle_screen_audio(move |enabled| {
         sysaudio_tx.send_replace(enabled);
     });
     let screen_enable_tx = screen_tx.clone();
     let weak_start = ui.as_weak();
     ui.on_start_screen(move || {
         if let Some(ui) = weak_start.upgrade() {
             let index = ui.get_screen_monitor_index().max(0) as usize;
             ui.set_screen_sharing(true);
             let profile = screen::VideoProfile::from_indices(
                 ui.get_video_resolution_index().max(0) as usize,
                 ui.get_video_fps_index().max(0) as usize,
             );
             ui.set_screen_status(format!("Iniciando H.264 {}...", profile.label()).into());
             screen_enable_tx.send_replace(Some(index));
         }
     });
     let screen_disable_tx = screen_tx.clone();
     let weak_stop = ui.as_weak();
     ui.on_stop_screen(move || {
         screen_disable_tx.send_replace(None);
         if let Some(ui) = weak_stop.upgrade() {
             ui.set_screen_sharing(false);
             ui.set_fullscreen_video(false);
             if ui.get_watching_stream_id().is_empty() {
                 ui.set_screen_status("Compartilhamento encerrado".into());
                 ui.set_screen_frame(slint::Image::default());
             }
         }
     });
     let weak_pick = ui.as_weak();
     ui.on_select_screen_monitor(move |index| {
         if let Some(ui) = weak_pick.upgrade() {
             ui.set_screen_monitor_index(index.max(0));
         }
     });
     let weak_refresh_screen = ui.as_weak();
     ui.on_refresh_screen_monitors(move || {
         if let Some(ui) = weak_refresh_screen.upgrade() {
             let model = slint::VecModel::from(screen::monitor_names().into_iter()
                 .map(slint::SharedString::from).collect::<Vec<_>>());
             ui.set_screen_monitors(slint::ModelRc::new(model));
             ui.set_screen_monitor_index(0);
         }
     });

    let watch_control = watch_tx.clone();
    let weak_watch = ui.as_weak();
    ui.on_watch_screen(move |peer_id| {
        let id = peer_id.to_string();
        watch_control.send_replace(Some(id.clone()));
        if let Some(ui) = weak_watch.upgrade() {
            ui.set_watching_stream_id(id.into());
            ui.set_active_section(3);
        }
    });
    let leave_control = watch_tx;
    let weak_leave = ui.as_weak();
    ui.on_leave_screen(move || {
        leave_control.send_replace(None);
        if let Some(ui) = weak_leave.upgrade() {
            ui.set_watching_stream_id("".into());
            ui.set_fullscreen_video(false);
            ui.set_screen_frame(slint::Image::default());
            ui.set_screen_status("Voce saiu da transmissao. A chamada continua.".into());
        }
    });
    ui.run()?;
    Ok(())
}
