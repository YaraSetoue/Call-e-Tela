mod audio;
mod p2p;

use std::{collections::HashMap, sync::Arc};

use futures_util::{SinkExt, StreamExt};
use p2p::{Mesh, MeshEvent};
use slint::{ComponentHandle, Model};
use tokio::sync::{mpsc, watch};
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

    export component MainWindow inherits Window {
        title: "Voice Screen - Salas de voz";
        preferred-width: 1060px;
        preferred-height: 720px;
        background: Theme.background;
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
        in-out property <string> p2p-text: "Aguardando conexao WebRTC";
        in-out property <bool> microphone-enabled: false;
        in-out property <[string]> microphone-list: ["Padrao do Windows"];
        in-out property <int> microphone-index: 0;
        in-out property <string> microphone-meter: "Microfone desligado";
        in-out property <[string]> output-list: ["Padrao do Windows"];
        in-out property <int> output-index: 0;
        in-out property <float> master-volume: 100;
        in-out property <[PeerMixRow]> peer-volumes: [];
        in-out property <string> audio-stats-text: "Aguardando participantes para iniciar a voz";
        in-out property <int> active-section: 0;

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

        VerticalLayout {
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

                        // Logs ficam numa aba separada, sem poluir a experiencia.
                        if root.active-section == 2 : ScrollView {
                            VerticalLayout {
                                padding: 28px;
                                spacing: 12px;
                                Text { text: "Diagnóstico da chamada"; font-size: 22px; font-weight: 600; color: Theme.text; }
                                Text { text: "Use estes detalhes apenas ao investigar falhas de audio ou conexao."; font-size: 13px; color: Theme.muted; wrap: word-wrap; }
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
    if log.len() > 7 { log.remove(0); }
}

async fn connection(
    weak: slint::Weak<MainWindow>,
    command: ConnectionCommand,
    mut mic_state: watch::Receiver<bool>,
    mut selected_mic: watch::Receiver<usize>,
    mut selected_output: watch::Receiver<usize>,
    mut speaker_test: watch::Receiver<u64>,
    mut cancel: watch::Receiver<bool>,
    playback: PlaybackBus,
) {
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
    let audio = AudioSession::new(event_tx.clone(), playback);
    let mesh = Arc::new(Mesh::new(
        signal_tx, event_tx.clone(), audio.frames.clone(), audio.playback.clone(),
    ));
    audio.output(*selected_output.borrow());
    audio.capture(*mic_state.borrow(), *selected_mic.borrow());
    let mut peers: HashMap<String, String> = HashMap::new();
    let mut voice_stats: HashMap<String, (u64, u64)> = HashMap::new();
    let mut log: Vec<String> = Vec::new();
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
                        room = Some(room_id);
                        status = "Sala criada. Aguarde novos participantes.".into();
                    }
                    ServerMessage::RoomJoined { room_id, peers: members, .. } => {
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
                        status = format!("{name} saiu da sala.");
                        mesh.leave(&peer_id).await;
                    }
                    ServerMessage::Signal { from, payload } => {
                        match payload {
                            SignalPayload::Offer { sdp } => {
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
                                if let Err(e) = mesh.apply_answer(&from, sdp).await {
                                    add_log(&mut log, format!("Resposta WebRTC invalida: {e}"));
                                }
                            }
                            SignalPayload::IceCandidate { .. } => {
                                // Este prototipo usa non-trickle ICE: os candidatos seguem no SDP.
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
                    MeshEvent::VoiceStats { peer, sent, received } => {
                        voice_stats.insert(peer, (sent, received));
                        update_voice_stats(&weak, &peers, &voice_stats);
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
    let playback = PlaybackBus::default();
    let microphones = microphone_names();
    let microphone_model = slint::VecModel::from(microphones.into_iter()
        .map(slint::SharedString::from).collect::<Vec<_>>());
    ui.set_microphone_list(slint::ModelRc::new(microphone_model));
    let outputs = output_device_names();
    let output_model = slint::VecModel::from(outputs.into_iter()
        .map(slint::SharedString::from).collect::<Vec<_>>());
    ui.set_output_list(slint::ModelRc::new(output_model));
    let weak = ui.as_weak();
    let session_playback = playback.clone();
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
                job = Some((stop, tokio::spawn(connection(
                    weak.clone(), command, mic, selected, output, test, cancel, session_playback.clone(),
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
    ui.run()?;
    Ok(())
}
