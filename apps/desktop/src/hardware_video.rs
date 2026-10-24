//! Backend experimental de baixa copia para Windows:
//! DXGI Desktop Duplication / D3D11 -> QSV VPP -> Intel Quick Sync H.264.
//! Usa ffmpeg.exe externo, sem alterar o protocolo ou os canais de audio.
//! A captura DXGI e diferente da API Windows Graphics Capture (WGC).
use std::{
    io::{BufRead, BufReader, Read},
    process::{Child, Command, Stdio},
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, mpsc};
use crate::p2p::MeshEvent;
use super::{VideoProfile, MAX_FRAME_BYTES};

const MAX_BUFFERED_BYTES: usize = 2 * MAX_FRAME_BYTES;

// Delimitadores Annex-B: o FFmpeg insere AUD (NAL type 9) por quadro.
fn start_code_at(data: &[u8], at: usize) -> Option<usize> {
    if at + 3 <= data.len() && data[at..].starts_with(&[0, 0, 1]) {
        Some(3)
    } else if at + 4 <= data.len() && data[at..].starts_with(&[0, 0, 0, 1]) {
        Some(4)
    } else { None }
}
fn find_start(data: &[u8], from: usize) -> Option<(usize, usize)> {
    if data.len() < 3 || from >= data.len() { return None; }
    (from..data.len().saturating_sub(2))
        .find_map(|i| start_code_at(data, i).map(|length| (i, length)))
}

#[derive(Default)]
struct H264AudParser {
    buffered: Vec<u8>,
    current: Vec<u8>,
    has_picture: bool,
    has_sps: bool,
    has_pps: bool,
    sps: Vec<u8>,
    pps: Vec<u8>,
}
impl H264AudParser {
    fn nal(&mut self, unit: &[u8], start_len: usize, frames: &mut Vec<Vec<u8>>) -> Result<(), String> {
        if unit.len() <= start_len { return Ok(()); }
        let nal_type = unit[start_len] & 0x1f;
        if nal_type == 9 { // AUD: inicio de access unit
            if self.has_picture && !self.current.is_empty() {
                frames.push(std::mem::take(&mut self.current));
            } else {
                self.current.clear();
            }
            self.has_picture = false;
            self.has_sps = false;
            self.has_pps = false;
        }
        if nal_type == 7 {
            self.sps = unit.to_vec();
            self.has_sps = true;
        } else if nal_type == 8 {
            self.pps = unit.to_vec();
            self.has_pps = true;
        } else if nal_type == 5 { // IDR deve ser decodificavel mesmo apos entrar tarde
            if !self.has_sps && !self.sps.is_empty() { self.current.extend_from_slice(&self.sps); }
            if !self.has_pps && !self.pps.is_empty() { self.current.extend_from_slice(&self.pps); }
        }
        if nal_type == 1 || nal_type == 5 { self.has_picture = true; }
        self.current.extend_from_slice(unit);
        if self.current.len() > MAX_FRAME_BYTES {
            return Err("Access unit H.264 excedeu MAX_FRAME_BYTES".into());
        }
        Ok(())
    }
    fn push(&mut self, data: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        self.buffered.extend_from_slice(data);
        let mut frames = Vec::new();
        loop {
            let Some((start, length)) = find_start(&self.buffered, 0) else { break; };
            if start != 0 { self.buffered.drain(..start); }
            let Some((next, _)) = find_start(&self.buffered, length) else { break; };
            let nal = self.buffered[..next].to_vec();
            self.nal(&nal, length, &mut frames)?;
            self.buffered.drain(..next);
        }
        if self.buffered.len() > MAX_BUFFERED_BYTES {
            return Err("Fluxo H.264 sem delimitadores Annex-B validos".into());
        }
        Ok(frames)
    }
}

/// Retorna Ok quando o compartilhamento foi interrompido pelo usuario.
/// Retorna Err quando o caminho QSV falha, permitindo cair no OpenH264 existente.
pub fn capture(
    index: usize,
    profile: VideoProfile,
    outgoing: broadcast::Sender<Vec<u8>>,
    events: mpsc::UnboundedSender<MeshEvent>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    let ffmpeg = std::env::var("VOICE_FFMPEG_PATH")
        .unwrap_or_else(|_| "ffmpeg.exe".to_string());
    // FFmpeg libavfilter ddagrab captura superfices D3D11; hwmap transforma
    // as texturas para o dominio QSV e vpp_qsv escala na GPU.
    let filter = format!(
        "ddagrab=output_idx={index}:framerate={},hwmap=derive_device=qsv,format=qsv,vpp_qsv=w={}:h={}[v]",
        profile.fps, profile.width, profile.height,
    );
    let bitrate = profile.bitrate_bps.to_string();
    let maxrate = bitrate.clone();
    let bufsize = (profile.bitrate_bps.saturating_mul(2)).to_string();
    let gop = profile.fps.saturating_mul(2).to_string();
    let mut process = Command::new(&ffmpeg)
        .args([
            "-hide_banner", "-loglevel", "error", "-nostdin",
            "-init_hw_device", "d3d11va=d3d",
            "-filter_hw_device", "d3d",
            "-filter_complex", &filter, "-map", "[v]",
            "-c:v", "h264_qsv", "-preset", "veryfast",
            "-b:v", &bitrate, "-maxrate", &maxrate, "-bufsize", &bufsize,
            "-g", &gop, "-bf", "0", "-repeat_pps", "1",
            "-bsf:v", "h264_metadata=aud=insert",
            "-f", "h264", "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Nao foi possivel executar '{ffmpeg}': {e}"))?;
    let stdout = process.stdout.take().ok_or("stdout FFmpeg indisponivel")?;
    let stderr = process.stderr.take().ok_or("stderr FFmpeg indisponivel")?;
    let child = Arc::new(Mutex::new(process));
    let done = Arc::new(AtomicBool::new(false));
    let last_frame = Arc::new(Mutex::new(Instant::now()));
    let stderr_lines = Arc::new(Mutex::new(Vec::<String>::new()));

    let (errors, event_copy) = (stderr_lines.clone(), events.clone());
    let stderr_thread = thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break; };
            if let Ok(mut logs) = errors.lock() {
                if logs.len() >= 6 { logs.remove(0); }
                logs.push(line.clone());
            }
            // Um trecho curto no diagnostico; evitar inundar a aba.
            if line.contains("Error") || line.contains("Failed") || line.contains("error") {
                let _ = event_copy.send(MeshEvent::Status(format!(
                    "FFmpeg QSV: {}", line.chars().take(180).collect::<String>(),
                )));
            }
        }
    });

    let (proc_ref, done_ref, stop_ref, frame_ref) =
        (child.clone(), done.clone(), stop.clone(), last_frame.clone());
    let watcher = thread::spawn(move || {
        while !done_ref.load(Ordering::Acquire) {
            let timed_out = frame_ref.lock()
                .map(|last| last.elapsed() > Duration::from_secs(8))
                .unwrap_or(true);
            if stop_ref.load(Ordering::Relaxed) || timed_out {
                if let Ok(mut proc) = proc_ref.lock() { let _ = proc.kill(); }
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    });

    let _ = events.send(MeshEvent::Status(format!(
        "QSV iniciando: Direct3D11/DXGI -> Intel Quick Sync, {}, {:.2} Mbps",
        profile.label(), profile.bitrate_bps as f64 / 1_000_000.0,
    )));
    // A previa do apresentador e separada (2 FPS): nao decodificar o mesmo
    // fluxo H.264 apenas para atualizar a janela local, nem limitar o envio.
    let preview_done = done.clone();
    let preview_stop = stop.clone();
    let preview_events = events.clone();
    let preview = thread::spawn(move || {
        use image::DynamicImage;
        let Some(monitor) = xcap::Monitor::all().ok()
            .and_then(|all| all.into_iter().nth(index)) else { return; };
        while !preview_done.load(Ordering::Relaxed)
            && !preview_stop.load(Ordering::Relaxed)
        {
            if let Ok(raw) = monitor.capture_image() {
                let image = DynamicImage::ImageRgba8(raw).thumbnail(640, 360).to_rgb8();
                let (width, height) = image.dimensions();
                let _ = preview_events.send(MeshEvent::ScreenRgbFrame {
                    peer: "__local__".into(), width, height, rgb: image.into_raw(),
                });
            }
            thread::sleep(Duration::from_millis(500));
        }
    });
    let mut source = BufReader::with_capacity(128 * 1024, stdout);
    let mut parser = H264AudParser::default();
    let mut buffer = [0u8; 64 * 1024];
    let mut count = 0u64;
    let mut bits = 0u64;
    let mut window = Instant::now();
    let read_result = loop {
        match source.read(&mut buffer) {
            Ok(0) => break Ok(()),
            Ok(size) => {
                let frames = match parser.push(&buffer[..size]) {
                    Ok(frames) => frames,
                    Err(error) => break Err(error),
                };
                for frame in frames {
                    if stop.load(Ordering::Relaxed) { break; }
                    if let Ok(mut last) = last_frame.lock() { *last = Instant::now(); }
                    bits += frame.len() as u64 * 8;
                    count += 1;
                    let mut message = Vec::with_capacity(frame.len() + 4);
                    message.extend_from_slice(b"VSHF");
                    message.extend_from_slice(&frame);
                    let _ = outgoing.send(message);
                    if count == 1 {
                        let _ = events.send(MeshEvent::Status(format!(
                            "QSV ativo: primeiro quadro H.264 enviado ({} bytes)", frame.len(),
                        )));
                    }
                }
                if window.elapsed() >= Duration::from_secs(5) {
                    let secs = window.elapsed().as_secs_f64();
                    let _ = events.send(MeshEvent::Status(format!(
                        "VIDEO ORIGEM QSV {}: {:.1} FPS codificados / {:.2} Mbps, captura DXGI + encoder GPU",
                        profile.label(), count as f64 / secs, bits as f64 / secs / 1_000_000.0,
                    )));
                    count = 0;
                    bits = 0;
                    window = Instant::now();
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => break Err(format!("Falha lendo fluxo H.264 do FFmpeg: {e}")),
        }
    };
    done.store(true, Ordering::Release);
    if let Ok(mut proc) = child.lock() {
        let _ = proc.kill();
        let _ = proc.wait();
    }
    let _ = watcher.join();
    let _ = stderr_thread.join();
    let _ = preview.join();
    if stop.load(Ordering::Relaxed) { return Ok(()); }
    let detail = stderr_lines.lock()
        .map(|lines| lines.join(" | ")).unwrap_or_default();
    // Sair espontaneamente do FFmpeg sem frames e sempre erro.
    Err(format!("{} ({detail})", read_result.err().unwrap_or_else(|| "FFmpeg finalizou".into())))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn nal(kind: u8, marker: u8) -> Vec<u8> {
        vec![0, 0, 0, 1, kind, marker]
    }
    #[test]
    fn splits_units_by_aud_even_across_reads() {
        let mut parser = H264AudParser::default();
        let mut stream = Vec::new();
        for (t, m) in [(9, 0xf0), (7, 1), (8, 2), (5, 3), (9, 0xf0), (1, 4), (9, 0xf0), (1, 5), (9, 0xf0)] {
            stream.extend(nal(t, m));
        }
        let mut frames = vec![];
        for b in stream.chunks(7) { frames.extend(parser.push(b).unwrap()); }
        assert_eq!(frames.len(), 2); // 3o quadro ainda nao foi fechado pelo proximo AUD.
        assert!(frames[0].windows(2).any(|w| w == [5, 3]));
    }
    #[test]
    fn oversized_nal_is_rejected() {
        let mut parser = H264AudParser::default();
        parser.push(&[0,0,0,1,5]).unwrap();
        assert!(parser.push(&vec![7; MAX_BUFFERED_BYTES]).is_err());
    }
}
