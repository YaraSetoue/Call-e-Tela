//! Captura e transmissao de tela MVP. Windows: XCap + JPEG.
//! Perfil atual 1280x720, ate 15 FPS. Para producao: H.264/RTP + GPU.
use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, thread, time::{Duration, Instant}};
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, ColorType, DynamicImage};
use tokio::sync::{broadcast, mpsc};
use crate::p2p::MeshEvent;

pub const CAP_WIDTH: u32 = 1280;
pub const CAP_HEIGHT: u32 = 720;
pub const CAP_FPS: u32 = 15;
pub const CHUNK_BYTES: usize = 8 * 1024;
pub const MAX_FRAME_BYTES: usize = 350_000;
pub const MAX_CHUNKS: usize = 48;
pub const HEADER_SIZE: usize = 12;

/// Perfil planejado, nao habilitado na interface nesta etapa.
#[allow(dead_code)]
pub const FUTURE_PROFILES: &[(u32,u32)] = &[
    (256, 144), (996, 560), (1280, 720), (1920, 1080), (2560, 1440),
];
#[allow(dead_code)]
pub const FUTURE_FPS: &[u32] = &[15, 30, 60];

pub fn monitor_names() -> Vec<String> {
    xcap::Monitor::all().map(|monitors| monitors.into_iter().enumerate().map(|(i, mon)| {
        format!("Monitor {} - {}", i + 1,
            mon.friendly_name().unwrap_or_else(|_| "Tela".to_owned()))
    }).collect()).unwrap_or_default()
}

pub struct CaptureHandle { stop: Arc<AtomicBool> }
impl CaptureHandle {
    pub fn start(
        index: usize,
        outgoing: broadcast::Sender<Vec<u8>>,
        events: mpsc::UnboundedSender<MeshEvent>,
    ) -> Result<Self, String> {
        if index >= xcap::Monitor::all().map_err(|e| e.to_string())?.len() {
            return Err("Monitor nao encontrado".to_owned());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        thread::spawn(move || {
            // Abrir o monitor dentro da thread evita transferir handles Win32 entre threads.
            let Some(mon) = xcap::Monitor::all().ok().and_then(|items| items.into_iter().nth(index)) else {
                let _ = events.send(MeshEvent::Status("Monitor indisponivel ao iniciar captura".into()));
                return;
            };
            let duration = Duration::from_nanos(1_000_000_000 / CAP_FPS as u64);
            let mut failures = 0u32;
            while !stopped.load(Ordering::Relaxed) {
                let started = Instant::now();
                let result = (|| -> Result<Vec<u8>, String> {
                    let raw = mon.capture_image().map_err(|e| e.to_string())?;
                    let img = DynamicImage::ImageRgba8(raw)
                        .resize(CAP_WIDTH, CAP_HEIGHT, FilterType::Triangle).to_rgb8();
                    let mut data = Vec::with_capacity(65_536);
                    let mut encoder = JpegEncoder::new_with_quality(&mut data, 48);
                    encoder.encode(img.as_raw(), img.width(), img.height(), ColorType::Rgb8.into())
                        .map_err(|e| e.to_string())?;
                    Ok(data)
                })();
                match result {
                    Ok(jpeg) if jpeg.len() <= MAX_FRAME_BYTES => {
                        failures = 0;
                        let _ = outgoing.send(jpeg.clone());
                        let _ = events.send(MeshEvent::ScreenFrame {
                            peer: "__local__".to_owned(), jpeg,
                        });
                    }
                    Ok(jpeg) => {
                        failures += 1;
                        if failures == 1 {
                            let _ = events.send(MeshEvent::Status(format!(
                                "Quadro de tela muito grande ({} bytes); quadro descartado", jpeg.len()
                            )));
                        }
                    }
                    Err(e) => {
                        failures += 1;
                        if failures == 1 {
                            let _ = events.send(MeshEvent::Status(format!("Falha capturando tela: {e}")));
                        }
                    }
                }
                if let Some(wait) = duration.checked_sub(started.elapsed()) { thread::sleep(wait); }
            }
        });
        Ok(Self { stop })
    }
}
impl Drop for CaptureHandle {
    fn drop(&mut self) { self.stop.store(true, Ordering::Relaxed); }
}

/// Pacote de JPEG: VSS1 + frame_id(u32 BE) + index(u16 BE) + count(u16 BE) + dados.
pub fn chunks(frame_id: u32, jpeg: &[u8]) -> Option<Vec<Vec<u8>>> {
    if jpeg.len() > MAX_FRAME_BYTES || jpeg.is_empty() { return None; }
    let total = jpeg.len().div_ceil(CHUNK_BYTES);
    if total > MAX_CHUNKS { return None; }
    Some(jpeg.chunks(CHUNK_BYTES).enumerate().map(|(i, payload)| {
        let mut result = Vec::with_capacity(HEADER_SIZE + payload.len());
        result.extend_from_slice(b"VSS1");
        result.extend_from_slice(&frame_id.to_be_bytes());
        result.extend_from_slice(&(i as u16).to_be_bytes());
        result.extend_from_slice(&(total as u16).to_be_bytes());
        result.extend_from_slice(payload);
        result
    }).collect())
}

pub struct Reassembler {
    id: Option<u32>,
    parts: Vec<Option<Vec<u8>>>,
    received: usize,
    bytes: usize,
    since: Instant,
}
impl Reassembler {
    pub fn new() -> Self {
        Self { id: None, parts: Vec::new(), received: 0, bytes: 0, since: Instant::now() }
    }
    pub fn push(&mut self, packet: &[u8]) -> Option<Vec<u8>> {
        if packet.len() < HEADER_SIZE || &packet[..4] != b"VSS1" { return None; }
        let id = u32::from_be_bytes(packet[4..8].try_into().ok()?);
        let index = u16::from_be_bytes(packet[8..10].try_into().ok()?) as usize;
        let total = u16::from_be_bytes(packet[10..12].try_into().ok()?) as usize;
        if total == 0 || total > MAX_CHUNKS || index >= total { return None; }
        if self.id != Some(id) || self.since.elapsed() > Duration::from_millis(350) {
            self.id = Some(id);
            self.parts = vec![None; total];
            self.received = 0;
            self.bytes = 0;
            self.since = Instant::now();
        }
        if self.parts.len() != total || self.parts[index].is_some() { return None; }
        self.bytes += packet.len() - HEADER_SIZE;
        if self.bytes > MAX_FRAME_BYTES {
            self.id = None;
            self.parts.clear();
            return None;
        }
        self.parts[index] = Some(packet[HEADER_SIZE..].to_vec());
        self.received += 1;
        if self.received != total { return None; }
        let mut jpeg = Vec::with_capacity(self.bytes);
        for part in &mut self.parts { jpeg.extend_from_slice(part.take()?.as_slice()); }
        self.id = None;
        self.parts.clear();
        Some(jpeg)
    }
}
