mod p2p;

use std::{collections::HashMap, sync::Arc};

use futures_util::{SinkExt, StreamExt};
use p2p::{Mesh, MeshEvent};
use slint::ComponentHandle;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use voice_protocol::{ClientMessage, ServerMessage, SignalPayload};

slint::slint! {
    import { Button, LineEdit } from "std-widgets.slint";
    export component MainWindow inherits Window {
        title: "Voice Screen - Etapa 2 (WebRTC)";
        width: 640px;
        height: 525px;
        in-out property <string> server-url: "wss://voz.virtuabil.cloud/ws";
        in-out property <string> display-name: "Participante";
        in-out property <string> room-code: "";
        in-out property <string> status-text: "Pronto para criar ou entrar em uma sala";
        in-out property <string> participants-text: "Nenhum participante conectado";
        in-out property <string> p2p-text: "O teste P2P iniciara quando um segundo participante entrar.";
        callback create-room();
        callback join-room();
        VerticalLayout {
            padding: 20px;
            spacing: 10px;
            Text { text: "Voice Screen"; font-size: 26px; font-weight: 700; }
            Text { text: "Servidor de sinalizacao:"; }
            LineEdit { text <=> root.server-url; }
            Text { text: "Seu nome:"; }
            LineEdit { text <=> root.display-name; }
            Text { text: "Codigo da sala:"; }
            LineEdit { text <=> root.room-code; }
            HorizontalLayout {
                spacing: 12px;
                Button { text: "Criar sala"; clicked => { root.create-room(); } }
                Button { text: "Entrar na sala"; clicked => { root.join-room(); } }
            }
            Text { text: root.status-text; wrap: word-wrap; }
            Text { text: root.participants-text; wrap: word-wrap; }
            Text { text: "Eventos P2P:"; font-weight: 700; }
            Rectangle {
                background: #f2f4f7;
                border-radius: 8px;
                VerticalLayout {
                    padding: 12px;
                    Text { text: root.p2p-text; wrap: word-wrap; color: #344054; }
                }
            }
            Text {
                text: "Prototipo: negocia WebRTC e envia saudacao por DataChannel. Sem voz/tela ainda. TURN nao configurado.";
                wrap: word-wrap;
                color: #667085;
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

fn add_log(log: &mut Vec<String>, message: String) {
    log.push(message);
    if log.len() > 7 { log.remove(0); }
}

async fn connection(weak: slint::Weak<MainWindow>, command: ConnectionCommand) {
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
    let mesh = Arc::new(Mesh::new(signal_tx, event_tx.clone()));
    let mut peers: HashMap<String, String> = HashMap::new();
    let mut log: Vec<String> = Vec::new();
    let mut status = "Conectado ao WebSocket".to_owned();
    let mut room = None;

    loop {
        tokio::select! {
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
                    ServerMessage::PeerLeft { peer_id } => {
                        let name = peers.remove(&peer_id).unwrap_or_else(|| "Participante".into());
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
                }
                update_ui(&weak, room.clone(), status.clone(), user_list(&peers), log.join("\n"));
            }
        }
    }
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
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all().build().expect("Erro criando runtime Tokio");
        runtime.block_on(async move {
            let mut job: Option<tokio::task::JoinHandle<()>> = None;
            while let Some(command) = rx.recv().await {
                if let Some(old) = job.take() { old.abort(); }
                job = Some(tokio::spawn(connection(weak.clone(), command)));
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
    ui.run()?;
    Ok(())
}
