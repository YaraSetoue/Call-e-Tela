use std::collections::HashMap;

use futures_util::{SinkExt, StreamExt};
use slint::ComponentHandle;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use voice_protocol::{ClientMessage, ServerMessage};

slint::slint! {
    import { Button, LineEdit } from "std-widgets.slint";
    export component MainWindow inherits Window {
        title: "Voice Screen - Etapa 1";
        width: 560px;
        height: 450px;
        in-out property <string> server-url: "ws://127.0.0.1:8080/ws";
        in-out property <string> display-name: "Participante";
        in-out property <string> room-code: "";
        in-out property <string> status-text: "Pronto para criar ou entrar em uma sala";
        in-out property <string> participants-text: "Nenhum participante conectado";
        callback create-room();
        callback join-room();
        VerticalLayout {
            padding: 20px;
            spacing: 12px;
            Text { text: "Voice Screen"; font-size: 26px; font-weight: 700; }
            Text { text: "Servidor de sinalizacao:"; }
            LineEdit { text <=> root.server-url; }
            Text { text: "Seu nome:"; }
            LineEdit { text <=> root.display-name; }
            Text { text: "Codigo da sala (para entrar ou compartilhar):"; }
            LineEdit { text <=> root.room-code; }
            HorizontalLayout {
                spacing: 12px;
                Button { text: "Criar sala"; clicked => { root.create-room(); } }
                Button { text: "Entrar na sala"; clicked => { root.join-room(); } }
            }
            Text { text: root.status-text; wrap: word-wrap; }
            Text { text: root.participants-text; wrap: word-wrap; }
            Text {
                text: "Esta versao testa apenas salas e sinalizacao. Voz/tela vem nas proximas etapas.";
                wrap: word-wrap;
                color: #667085;
            }
        }
    }
}

#[derive(Clone)]
struct ConnectionCommand { url: String, message: ClientMessage }

fn update_ui(weak: &slint::Weak<MainWindow>, room: Option<String>, status: String, users: String) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        if let Some(room) = room { ui.set_room_code(room.into()); }
        ui.set_status_text(status.into());
        ui.set_participants_text(users.into());
    });
}

async fn connection(weak: slint::Weak<MainWindow>, command: ConnectionCommand) {
    update_ui(&weak, None, "Conectando ao servidor...".into(), "".into());
    let (socket, _) = match connect_async(&command.url).await {
        Ok(pair) => pair,
        Err(e) => {
            update_ui(&weak, None, format!("Falha de conexao: {e}"), "".into());
            return;
        }
    };
    let (mut sink, mut stream) = socket.split();
    let json = match serde_json::to_string(&command.message) {
        Ok(v) => v,
        Err(e) => {
            update_ui(&weak, None, format!("Erro serializando mensagem: {e}"), "".into());
            return;
        }
    };
    if let Err(e) = sink.send(Message::Text(json.into())).await {
        update_ui(&weak, None, format!("Falha ao enviar: {e}"), "".into());
        return;
    }
    let mut peers: HashMap<String, String> = HashMap::new();
    while let Some(msg) = stream.next().await {
        let msg = match msg {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(e) => {
                update_ui(&weak, None, format!("Conexao interrompida: {e}"), "".into());
                return;
            }
        };
        let Ok(event) = serde_json::from_str::<ServerMessage>(&msg) else { continue; };
        let mut new_room = None;
        let status = match event {
            ServerMessage::RoomCreated { room_id, .. } => {
                new_room = Some(room_id);
                "Sala criada! Compartilhe o codigo com outros usuarios.".to_string()
            }
            ServerMessage::RoomJoined { room_id, peers: members, .. } => {
                new_room = Some(room_id);
                for peer in members { peers.insert(peer.id, peer.name); }
                "Voce entrou na sala com sucesso.".to_string()
            }
            ServerMessage::PeerJoined { peer } => {
                let name = peer.name.clone();
                peers.insert(peer.id, peer.name);
                format!("{name} entrou na sala.")
            }
            ServerMessage::PeerLeft { peer_id } => {
                let name = peers.remove(&peer_id).unwrap_or_else(|| "Participante".into());
                format!("{name} saiu da sala.")
            }
            ServerMessage::Error { code, message } => format!("Erro {code}: {message}"),
            ServerMessage::Signal { .. } => "Sinalizacao WebRTC recebida (midia ainda nao implementada).".into(),
        };
        let mut names: Vec<&String> = peers.values().collect();
        names.sort();
        let users = if names.is_empty() {
            "Voce e o primeiro participante.".into()
        } else {
            format!("Outros participantes: {}", names.into_iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))
        };
        update_ui(&weak, new_room, status, users);
    }
    update_ui(&weak, None, "Desconectado da sala.".into(), "".into());
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
