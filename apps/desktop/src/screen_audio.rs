//! Captura opcional do som do computador Windows via WASAPI process loopback.
//! Exclui o audio produzido pelo proprio processo (voz de outros participantes)
//! para evitar que a chamada seja retransmitida com eco.
//! Opus mono 48 kHz, 20 ms, pacote VSD1 + seq (u32 BE) + Opus.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc as std_mpsc, Arc,
    },
    thread,
    time::Duration,
};

use tokio::sync::{broadcast, mpsc};
use crate::p2p::MeshEvent;

#[cfg(windows)]
use std::collections::VecDeque;
#[cfg(windows)]
use opus::{Application, Channels, Encoder};
#[cfg(windows)]
use wasapi::{AudioClient, Direction, SampleType, StreamMode, WaveFormat};

#[cfg(windows)]
const FRAME: usize = 960;
#[cfg(windows)]
const CHANNELS: usize = 2;
#[cfg(windows)]
const FRAME_BYTES: usize = CHANNELS * 4; // f32 stereo

pub struct CaptureHandle {
    stop: Arc<AtomicBool>,
}

impl CaptureHandle {
    pub fn start(
        outgoing: broadcast::Sender<Vec<u8>>,
        events: mpsc::UnboundedSender<MeshEvent>,
    ) -> Result<Self, String> {
        #[cfg(not(windows))]
        {
            let _ = (outgoing, events);
            return Err("Som do computador disponivel apenas no Windows".to_owned());
        }
        #[cfg(windows)]
        {
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = stop.clone();
            let (ready_tx, ready_rx) = std_mpsc::sync_channel::<Result<(), String>>(1);
            thread::Builder::new()
                .name("voice-screen-wasapi-loopback".to_owned())
                .spawn(move || {
                    if let Err(err) = capture_windows(worker_stop, outgoing, events.clone(), ready_tx) {
                        let _ = events.send(MeshEvent::Status(format!(
                            "Falha capturando som da tela: {err}"
                        )));
                    }
                })
                .map_err(|e| format!("Nao foi possivel criar thread WASAPI: {e}"))?;
            match ready_rx.recv_timeout(Duration::from_secs(6)) {
                Ok(Ok(())) => Ok(Self { stop }),
                Ok(Err(err)) => {
                    stop.store(true, Ordering::Release);
                    Err(err)
                }
                Err(_) => {
                    stop.store(true, Ordering::Release);
                    Err("Timeout abrindo captura de audio do Windows".to_owned())
                }
            }
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(windows)]
fn capture_windows(
    stop: Arc<AtomicBool>,
    outgoing: broadcast::Sender<Vec<u8>>,
    events: mpsc::UnboundedSender<MeshEvent>,
    ready: std_mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    // COM + WASAPI devem ser inicializados e destruidos nesta mesma thread.
    let mut ready = Some(ready);
    let result = (|| -> Result<(), String> {
        wasapi::initialize_mta()
            .ok()
            .map_err(|e| format!("COM MTA: {e}"))?;
        // `false` mapeia para PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE.
        // Captura audio do sistema EXCETO o som reproduzido por este processo.
        // Requer Windows 10 build 20348+ / Windows 11.
        let mut client = AudioClient::new_application_loopback_client(
            std::process::id(), false,
        ).map_err(|e| format!("WASAPI process loopback: {e}. Requer Windows atualizado"))?;

        let format = WaveFormat::new(32, 32, &SampleType::Float, 48_000, CHANNELS, None);
        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: 0,
        };
        client.initialize_client(&format, &Direction::Capture, &mode)
            .map_err(|e| format!("WASAPI initialize_client: {e}"))?;
        let event_handle = client.set_get_eventhandle()
            .map_err(|e| format!("WASAPI eventhandle: {e}"))?;
        let capture = client.get_audiocaptureclient()
            .map_err(|e| format!("WASAPI capture client: {e}"))?;
        let mut encoder = Encoder::new(48_000, Channels::Mono, Application::Audio)
            .map_err(|e| format!("Opus do sistema: {e}"))?;
        encoder.set_bitrate(opus::Bitrate::Bits(80_000))
            .map_err(|e| format!("Opus bitrate: {e}"))?;
        client.start_stream().map_err(|e| format!("WASAPI start_stream: {e}"))?;

        if let Some(tx) = ready.take() { let _ = tx.send(Ok(())); }
        let _ = events.send(MeshEvent::Status(
            "Som da tela WASAPI iniciado (48 kHz, excluindo audio da chamada)".to_owned()
        ));
        let mut bytes = VecDeque::<u8>::with_capacity(FRAME * FRAME_BYTES * 6);
        let mut frame = [0.0f32; FRAME];
        let mut encoded = [0u8; 1500];
        let mut sequence = 0u32;
        let mut confirmed = false;
        while !stop.load(Ordering::Acquire) {
            // Timeout finito para o botao "Parar tela" responder rapidamente.
            if event_handle.wait_for_event(100).is_err() {
                continue;
            }
            loop {
                match capture.get_next_packet_size() {
                    Ok(Some(0)) | Ok(None) => break,
                    Ok(Some(_)) => {
                        capture.read_from_device_to_deque(&mut bytes)
                            .map_err(|e| format!("WASAPI leitura: {e}"))?;
                    }
                    Err(e) => return Err(format!("WASAPI tamanho do pacote: {e}")),
                }
            }
            // Nao acumular mais de ~200ms de audio em eventuais atrasos.
            const LIMIT: usize = FRAME * FRAME_BYTES * 10;
            while bytes.len() > LIMIT {
                for _ in 0..FRAME_BYTES { let _ = bytes.pop_front(); }
            }
            while bytes.len() >= FRAME * FRAME_BYTES && !stop.load(Ordering::Acquire) {
                for sample in &mut frame {
                    let mut left = [0u8; 4];
                    let mut right = [0u8; 4];
                    for byte in &mut left { *byte = bytes.pop_front().unwrap_or(0); }
                    for byte in &mut right { *byte = bytes.pop_front().unwrap_or(0); }
                    let a = f32::from_le_bytes(left);
                    let b = f32::from_le_bytes(right);
                    // Downmix estereo -> mono, sem alterar a voz da chamada.
                    *sample = if a.is_finite() && b.is_finite() {
                        ((a + b) * 0.5).clamp(-1.0, 1.0)
                    } else { 0.0 };
                }
                let len = encoder.encode_float(&frame, &mut encoded)
                    .map_err(|e| format!("Opus som da tela: {e}"))?;
                let mut packet = Vec::with_capacity(len + 8);
                packet.extend_from_slice(b"VSD1");
                packet.extend_from_slice(&sequence.to_be_bytes());
                packet.extend_from_slice(&encoded[..len]);
                sequence = sequence.wrapping_add(1);
                if outgoing.send(packet).is_ok() && !confirmed {
                    confirmed = true;
                    let _ = events.send(MeshEvent::Status(
                        "Som da tela ENVIADO para os canais WebRTC".to_owned()
                    ));
                }
            }
        }
        let _ = client.stop_stream();
        let _ = events.send(MeshEvent::Status("Som da tela WASAPI encerrado".to_owned()));
        Ok(())
    })();
    if let Some(tx) = ready.take() {
        let _ = tx.send(result.as_ref().map(|_| ()).map_err(|e| e.clone()));
    }
    result
}
