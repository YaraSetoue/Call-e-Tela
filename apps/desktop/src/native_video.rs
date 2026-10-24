//! Captura e H.264 nativos do Windows, sem ffmpeg.exe:
//! Windows.Graphics.Capture -> D3D11 NV12 -> Media Foundation hardware MFT.
//!
//! O crate moq-video encapsula essas APIs nativas; ele nao inicia processo
//! externo. Este modulo integra os access units Annex-B ao wire format VSHF
//! ja usado pelo Voice Screen. Intel/AMD/NVIDIA compartilham o mesmo caminho.
use std::{
    sync::{atomic::{AtomicBool, Ordering}, Arc},
    thread,
    time::{Duration, Instant},
};

use moq_video::{capture, encode, Rate, Size};
use tokio::sync::{broadcast, mpsc};

use crate::p2p::MeshEvent;
use super::{VideoProfile, MAX_FRAME_BYTES};

/// Retorna Ok somente quando a captura foi interrompida pelo usuario.
/// Se WGC / GPU / encoder falhar, retorna Err para acionar o OpenH264 existente.
pub fn capture(
    index: usize,
    profile: VideoProfile,
    outgoing: broadcast::Sender<Vec<u8>>,
    events: mpsc::UnboundedSender<MeshEvent>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    // A captura, D3D11 e o encoder precisam viver na mesma thread de GPU/COM.
    // O runtime current_thread evita mover o encoder !Send entre workers.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("Runtime da captura nativa: {err}"))?;

    // A previa do proprio apresentador usa uma captura leve e SEPARADA (1 FPS),
    // sem trazer cada quadro da transmissao de volta da GPU para a CPU.
    // Isso evita afetar o throughput do encoder principal.
    let preview_active = Arc::new(AtomicBool::new(true));
    let preview_thread = {
        let active = preview_active.clone();
        let preview_stop = stop.clone();
        let preview_events = events.clone();
        thread::spawn(move || {
            use image::DynamicImage;
            let Some(monitor) = xcap::Monitor::all().ok()
                .and_then(|monitors| monitors.into_iter().nth(index)) else { return; };
            while active.load(Ordering::Relaxed)
                && !preview_stop.load(Ordering::Relaxed)
            {
                if let Ok(rgba) = monitor.capture_image() {
                    let thumbnail = DynamicImage::ImageRgba8(rgba)
                        .thumbnail(640, 360).to_rgb8();
                    let (width, height) = thumbnail.dimensions();
                    let _ = preview_events.send(MeshEvent::ScreenRgbFrame {
                        peer: "__local__".to_string(),
                        width,
                        height,
                        rgb: thumbnail.into_raw(),
                    });
                }
                thread::sleep(Duration::from_secs(1));
            }
        })
    };

    let result = runtime.block_on(run_capture(index, profile, outgoing, events, stop));
    preview_active.store(false, Ordering::Relaxed);
    let _ = preview_thread.join();
    result
}

async fn run_capture(
    index: usize,
    profile: VideoProfile,
    outgoing: broadcast::Sender<Vec<u8>>,
    events: mpsc::UnboundedSender<MeshEvent>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    let fps = Rate::new(profile.fps, 1)
        .map_err(|err| format!("FPS invalido: {err}"))?;
    let size = Size::new(profile.width, profile.height);

    let mut capture_config = capture::Config::default();
    // WGC usa display:N como identificador de monitor. A ordem pode diferir
    // daquela de xcap em maquinas com varios monitores; conferir no teste.
    capture_config.source = capture::Source::Display(Some(format!("display:{index}")));
    capture_config.framerate = Some(fps);

    // Exigir hardware impede que um fallback interno silencioso seja anunciado
    // como "GPU". Nosso proprio screen.rs faz o fallback OpenH264 (CPU).
    let mut encoder_config = encode::Config::new(profile.width, profile.height, fps);
    encoder_config.kind = encode::Kind::Hardware;
    // A implementacao nativa resolve seu proprio bitrate inicial.
    // Controle dinamico por congestionamento sera conectado em uma etapa futura.

    let mut input = capture::open(&capture_config).await
        .map_err(|err| format!("WGC nao abriu monitor {index}: {err}"))?;
    let mut encoder = encode::Encoder::new(&encoder_config)
        .map_err(|err| format!("Encoder H.264 Media Foundation: {err}"))?;

    let name = encoder.name().to_string();
    let _ = events.send(MeshEvent::Status(format!(
        "NATIVE GPU H.264 ativo: WGC/D3D11 -> {name}, {} (sem FFmpeg)",
        profile.label(),
    )));

    let min_interval = Duration::from_secs_f64(1.0 / profile.fps.max(1) as f64);
    let mut last_submit = Instant::now() - min_interval;
    let mut first = true;
    let mut window = Instant::now();
    let mut read_count = 0u64;
    let mut submitted = 0u64;
    let mut encoded_count = 0u64;
    let mut total_bytes = 0u64;

    while !stop.load(Ordering::Relaxed) {
        // O timeout permite responder ao botao "Parar compartilhamento"
        // mesmo quando o desktop esta parado ou minimizado.
        let frame = match tokio::time::timeout(
            Duration::from_millis(500), input.read(),
        ).await {
            Ok(Ok(Some(frame))) => frame,
            Ok(Ok(None)) => {
                return Err("Captura WGC encerrada pelo Windows (monitor/resize)".into());
            }
            Ok(Err(err)) => return Err(format!("Falha recebendo quadro WGC: {err}")),
            Err(_) => continue,
        };
        read_count += 1;
        if last_submit.elapsed() < min_interval { continue; }
        last_submit = Instant::now();

        // Conversao NV12 e redimensionamento ficam na GPU quando a superficie
        // WGC e o encoder Media Foundation compartilham o dispositivo D3D11.
        let frame = if frame.size() != size {
            frame.resize(size, &moq_video::resize::Config::default())
                .map_err(|err| format!("Resize nativo: {err}"))?
        } else {
            frame
        };
        submitted += 1;

        // Uma unidade de acesso Annex-B por quadro codificado; SPS/PPS sao
        // inseridos no proprio fluxo para espectadores entrando tardiamente.
        let access_units = encoder.encode(&frame)
            .map_err(|err| format!("Media Foundation H264: {err}"))?;
        for access_unit in access_units {
            let payload = access_unit.payload.as_ref();
            if payload.is_empty() { continue; }
            if payload.len() > MAX_FRAME_BYTES {
                return Err(format!(
                    "Quadro H264 de {} bytes excede limite da rede ({} bytes)",
                    payload.len(), MAX_FRAME_BYTES
                ));
            }
            let mut message = Vec::with_capacity(payload.len() + 4);
            message.extend_from_slice(b"VSHF");
            message.extend_from_slice(payload);
            let _ = outgoing.send(message);
            encoded_count += 1;
            total_bytes += payload.len() as u64;
            if first {
                first = false;
                let _ = events.send(MeshEvent::Status(format!(
                    "NATIVE GPU: primeiro quadro H.264 enviado ({} bytes)", payload.len()
                )));
            }
        }
        if window.elapsed() >= Duration::from_secs(5) {
            let seconds = window.elapsed().as_secs_f64();
            let _ = events.send(MeshEvent::Status(format!(
                "VIDEO ORIGEM NATIVE {}: WGC {:.1} FPS, enviados ao encoder {:.1} FPS, H264 {:.1} FPS, {:.2} Mbps (GPU: {})",
                profile.label(),
                read_count as f64 / seconds,
                submitted as f64 / seconds,
                encoded_count as f64 / seconds,
                total_bytes as f64 * 8.0 / seconds / 1_000_000.0,
                name,
            )));
            window = Instant::now();
            read_count = 0;
            submitted = 0;
            encoded_count = 0;
            total_bytes = 0;
        }
    }
    Ok(())
}
