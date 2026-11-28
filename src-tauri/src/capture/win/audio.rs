//! Som de um processo (janela) ou de tudo menos este app (tela inteira), com o
//! WASAPI process loopback.
//!
//! Na tela inteira com apps mutados, o Windows não ajuda: ele só sabe tirar
//! um processo da captura do sistema, e esse lugar é do próprio AbreTela.
//! Então o som é montado ao contrário: uma captura para cada app com som
//! aberto que não está mutado, somadas num som só (`Mixer`). A lista é
//! conferida a cada poucos segundos, para pegar quem começou a tocar depois.

use super::sessions::{app_id, plan, processes, sessions};
use crate::capture::contracts::{AudioApp, AudioSource, AudioTarget, BoxError, FrameSink};
use crate::capture::packet::audio_packet;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

/// 10 ms de som estéreo a 48 kHz, em amostras (L e R contam separado).
const CHUNK: usize = 480 * 2;
/// Uma captura só entra na soma depois de juntar isto (30 ms): absorve a
/// diferença de ritmo entre elas sem estalo.
const PRIME: usize = CHUNK * 3;
/// Mais que isto esperando (100 ms) e a captura volta para perto do `PRIME`.
const MAX_QUEUE: usize = CHUNK * 10;
/// De quanto em quanto tempo conferir quem está com som aberto.
const RESCAN: Duration = Duration::from_secs(2);

pub struct WasapiLoopback;

impl AudioSource for WasapiLoopback {
    fn run(&self, target: AudioTarget, sink: Arc<dyn FrameSink>, stop: Arc<AtomicBool>) -> Result<(), BoxError> {
        use wasapi::*;
        let AudioTarget { pid, include_tree } = target;
        let _ = initialize_mta();

        let format = WaveFormat::new(32, 32, &SampleType::Float, 48000, 2, None);
        let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 };

        let client = match AudioClient::new_application_loopback_client(pid, include_tree)
            .and_then(|mut c| c.initialize_client(&format, &Direction::Capture, &mode).map(|_| c))
        {
            Ok(c) => {
                log_info!(
                    "audio",
                    "Capturing audio {}",
                    if include_tree { "only from the shared app" } else { "from the whole PC except AbreTela" }
                );
                c
            }
            Err(e) => {
                // Windows antigo sem process loopback: cai para o áudio do sistema todo
                log_warn!(
                    "audio",
                    "This Windows can't capture audio from a single app ({e}). Falling back to all PC audio,                  so viewers may also hear other sounds from this PC"
                );
                let device = DeviceEnumerator::new()?.get_default_device(&Direction::Render)?;
                let mut c = device.get_iaudioclient()?;
                c.initialize_client(&format, &Direction::Capture, &mode)?;
                c
            }
        };

        let event = client.set_get_eventhandle()?;
        let capture = client.get_audiocaptureclient()?;
        client.start_stream()?;

        const CHUNK: usize = 480 * 8; // 10ms de f32 estéreo a 48kHz
        let mut queue: VecDeque<u8> = VecDeque::new();
        while !stop.load(Ordering::Relaxed) && !sink.is_closed() {
            let _ = event.wait_for_event(100);
            while capture.get_next_packet_size()?.unwrap_or(0) > 0 {
                let before = queue.len();
                let info = capture.read_from_device_to_deque(&mut queue)?;
                if info.flags.silent {
                    queue.range_mut(before..).for_each(|b| *b = 0);
                }
            }
            while queue.len() >= CHUNK {
                sink.audio(audio_packet(queue.drain(..CHUNK)));
            }
        }
        let _ = client.stop_stream();
        Ok(())
    }

    fn apps(&self) -> Vec<AudioApp> {
        let _ = wasapi::initialize_mta();
        super::sessions::apps_from(&sessions(), &processes(), std::process::id())
    }

    fn run_muting(
        &self,
        target: AudioTarget,
        muted: &[String],
        sink: Arc<dyn FrameSink>,
        stop: Arc<AtomicBool>,
    ) -> Result<(), BoxError> {
        use wasapi::*;
        if muted.is_empty() || target.include_tree {
            return self.run(target, sink, stop);
        }
        let _ = initialize_mta();
        let format = WaveFormat::new(32, 32, &SampleType::Float, 48000, 2, None);
        let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 };
        let me = std::process::id();
        log_info!(
            "audio",
            "Capturing audio from the whole PC except AbreTela and the apps muted for viewers ({})",
            muted.join(", ")
        );

        let mut live: HashMap<u32, AppCapture> = HashMap::new();
        let mut mixer = Mixer::default();
        let mut failed: HashSet<u32> = HashSet::new();
        let mut reported: HashSet<String> = HashSet::new();
        let mut last_scan: Option<Instant> = None;
        let started = Instant::now();
        let mut emitted: u64 = 0;

        while !stop.load(Ordering::Relaxed) && !sink.is_closed() {
            if last_scan.is_none_or(|t| t.elapsed() >= RESCAN) {
                let first = last_scan.is_none();
                last_scan = Some(Instant::now());
                let procs = processes();
                let plan = plan(&sessions(), &procs, me, muted);
                let name = |pid: u32| procs.get(&pid).map_or_else(|| format!("process {pid}"), |p| app_id(&p.exe));
                live.retain(|pid, capture| {
                    let keep = plan.capture.contains(pid);
                    if !keep {
                        let _ = capture.client.stop_stream();
                        mixer.remove(*pid);
                    }
                    keep
                });
                for &pid in &plan.capture {
                    if live.contains_key(&pid) || failed.contains(&pid) {
                        continue;
                    }
                    match AppCapture::open(pid, &format, &mode) {
                        Ok(capture) => {
                            live.insert(pid, capture);
                        }
                        Err(e) => {
                            failed.insert(pid);
                            log_warn!("audio", "Could not capture the sound of {} ({e}), so viewers won't hear it", name(pid));
                        }
                    }
                }
                // Windows antigo sem process loopback: o jeito de sempre, com os mutados juntos
                if first && live.is_empty() && !plan.capture.is_empty() {
                    log_warn!(
                        "audio",
                        "This Windows can't capture apps one by one, so muting apps isn't possible here and viewers hear every app"
                    );
                    return self.run(target, sink, stop);
                }
                for app in plan.left_out {
                    if reported.insert(app.clone()) {
                        log_info!("audio", "{app} opened an app that is muted for viewers, so viewers won't hear {app} either");
                    }
                }
            }

            match live.values().next() {
                Some(capture) => {
                    let _ = capture.event.wait_for_event(5);
                }
                None => thread::sleep(Duration::from_millis(5)),
            }

            let mut gone = Vec::new();
            for (&pid, capture) in live.iter_mut() {
                match capture.read() {
                    Ok(()) => mixer.push(pid, capture.take_samples()),
                    Err(_) => gone.push(pid),
                }
            }
            for pid in gone {
                // o app fechou: sai da soma (e volta na próxima conferência, se abrir de novo)
                live.remove(&pid);
                mixer.remove(pid);
            }

            let due = (started.elapsed().as_micros() / 10_000) as u64;
            if due.saturating_sub(emitted) > 20 {
                // a thread ficou parada (PC travado): não despeja o atraso de uma vez
                emitted = due - 1;
            }
            while emitted < due {
                let chunk = mixer.next_chunk();
                sink.audio(audio_packet(chunk.iter().flat_map(|s| s.to_le_bytes())));
                emitted += 1;
            }
        }
        for capture in live.values() {
            let _ = capture.client.stop_stream();
        }
        Ok(())
    }
}

/// A captura do som de um app (com os processos que ele abriu).
struct AppCapture {
    client: wasapi::AudioClient,
    capture: wasapi::AudioCaptureClient,
    event: wasapi::Handle,
    bytes: VecDeque<u8>,
}

impl AppCapture {
    fn open(pid: u32, format: &wasapi::WaveFormat, mode: &wasapi::StreamMode) -> Result<Self, BoxError> {
        use wasapi::{AudioClient, Direction};
        let mut client = AudioClient::new_application_loopback_client(pid, true)?;
        client.initialize_client(format, &Direction::Capture, mode)?;
        let event = client.set_get_eventhandle()?;
        let capture = client.get_audiocaptureclient()?;
        client.start_stream()?;
        Ok(Self { client, capture, event, bytes: VecDeque::new() })
    }

    /// Lê tudo o que o Windows já tem.
    fn read(&mut self) -> Result<(), BoxError> {
        while self.capture.get_next_packet_size()?.unwrap_or(0) > 0 {
            let before = self.bytes.len();
            let info = self.capture.read_from_device_to_deque(&mut self.bytes)?;
            if info.flags.silent {
                self.bytes.range_mut(before..).for_each(|b| *b = 0);
            }
        }
        Ok(())
    }

    /// As amostras completas lidas até agora.
    fn take_samples(&mut self) -> Vec<f32> {
        let whole = self.bytes.len() / 4 * 4;
        let bytes: Vec<u8> = self.bytes.drain(..whole).collect();
        bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
    }
}

/// Soma o som de várias capturas, 10 ms por vez. Cada uma tem a própria fila:
/// só entra na soma depois de juntar 30 ms e, se esvaziar, sai até juntar de
/// novo. Assim uma captura atrasada falha sozinha, sem estalar as outras.
#[derive(Default)]
struct Mixer {
    inputs: HashMap<u32, Input>,
}

#[derive(Default)]
struct Input {
    queue: VecDeque<f32>,
    primed: bool,
}

impl Mixer {
    fn push(&mut self, id: u32, samples: Vec<f32>) {
        if samples.is_empty() {
            return;
        }
        let input = self.inputs.entry(id).or_default();
        input.queue.extend(samples);
        if input.queue.len() > MAX_QUEUE {
            let extra = input.queue.len() - PRIME;
            input.queue.drain(..extra);
        }
        if input.queue.len() >= PRIME {
            input.primed = true;
        }
    }

    fn remove(&mut self, id: u32) {
        self.inputs.remove(&id);
    }

    /// Os próximos 10 ms da soma (silêncio onde ninguém tem som).
    fn next_chunk(&mut self) -> Vec<f32> {
        let mut out = vec![0f32; CHUNK];
        for input in self.inputs.values_mut() {
            if !input.primed {
                continue;
            }
            let n = input.queue.len().min(CHUNK);
            for (o, s) in out.iter_mut().zip(input.queue.drain(..n)) {
                *o += s;
            }
            if n < CHUNK {
                input.primed = false;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(value: f32, samples: usize) -> Vec<f32> {
        vec![value; samples]
    }

    #[test]
    fn two_apps_are_summed() {
        let mut mixer = Mixer::default();
        mixer.push(1, tone(0.25, PRIME));
        mixer.push(2, tone(0.5, PRIME));
        assert!(mixer.next_chunk().iter().all(|&s| (s - 0.75).abs() < 1e-6));
    }

    #[test]
    fn an_app_joins_only_after_30_ms() {
        let mut mixer = Mixer::default();
        mixer.push(1, tone(0.5, CHUNK * 2));
        assert!(mixer.next_chunk().iter().all(|&s| s == 0.0), "not enough yet");
        mixer.push(1, tone(0.5, CHUNK));
        assert!(mixer.next_chunk().iter().all(|&s| s == 0.5));
    }

    #[test]
    fn an_app_that_runs_dry_waits_to_fill_again() {
        let mut mixer = Mixer::default();
        mixer.push(1, tone(0.5, PRIME + CHUNK / 2));
        for _ in 0..3 {
            mixer.next_chunk();
        }
        // só meio pedaço: entra o que tem e a captura sai da soma
        let last = mixer.next_chunk();
        assert_eq!(last.iter().filter(|&&s| s == 0.5).count(), CHUNK / 2);
        mixer.push(1, tone(0.5, CHUNK));
        assert!(mixer.next_chunk().iter().all(|&s| s == 0.0), "waits to fill again");
    }

    /// Guarda o som que chega, para medir.
    #[derive(Default)]
    struct Collect(std::sync::Mutex<Vec<f32>>);

    impl FrameSink for Collect {
        fn video(&self, _: crate::capture::packet::VideoPacket) {}
        fn audio(&self, packet: axum::body::Bytes) {
            let samples = packet[crate::capture::packet::HEADER..]
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            self.0.lock().unwrap().extend(samples);
        }
        fn is_closed(&self) -> bool {
            false
        }
    }

    /// A força de uma frequência no canal esquerdo (Goertzel).
    fn power(samples: &[f32], freq: f32) -> f32 {
        let k = 2.0 * (std::f32::consts::TAU * freq / 48_000.0).cos();
        let (mut a, mut b) = (0f32, 0f32);
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        for &x in &left {
            let c = x + k * a - b;
            b = a;
            a = c;
        }
        (a * a + b * b - k * a * b).sqrt() / left.len().max(1) as f32
    }

    /// Um WAV de `seconds` com um tom bem baixo.
    fn write_tone(path: &std::path::Path, freq: f32, seconds: u32) {
        let n = 48_000 * seconds;
        let mut wav = Vec::new();
        let data_len = n * 4;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..n {
            let s = ((i as f32 * freq * std::f32::consts::TAU / 48_000.0).sin() * 0.02 * 32767.0) as i16;
            wav.extend_from_slice(&s.to_le_bytes());
            wav.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, wav).unwrap();
    }

    /// Manual (toca dois tons baixinhos neste PC): o PowerShell 5 toca 440 Hz e
    /// o PowerShell 7 toca 660 Hz; a captura da tela inteira mutando um, outro
    /// ou os dois só leva o som de quem não foi mutado.
    #[test]
    #[ignore]
    fn muting_apps_on_this_pc() {
        let dir = std::env::temp_dir();
        let (a, b) = (dir.join("abretela-tone-440.wav"), dir.join("abretela-tone-660.wav"));
        write_tone(&a, 440.0, 20);
        write_tone(&b, 660.0, 20);
        // abertos pelo `start` do cmd, e não por este teste: o que este processo
        // abre conta como o próprio AbreTela e nunca entra no som
        let play = |exe: &str, wav: &std::path::Path| {
            let command = format!("(New-Object Media.SoundPlayer '{}').PlaySync()", wav.display());
            std::process::Command::new("cmd")
                .args(["/c", "start", "", "/min", exe, "-NoProfile", "-Command", &command])
                .status()
                .unwrap();
        };
        play("powershell", &a);
        play("pwsh", &b);
        std::thread::sleep(Duration::from_secs(4));

        let apps = WasapiLoopback.apps();
        println!("apps with sound: {apps:?}");
        let mut results = Vec::new();
        for muted in [vec![], vec!["pwsh".to_string()], vec!["powershell".to_string()], vec!["powershell".into(), "pwsh".into()]] {
            let sink = Arc::new(Collect::default());
            let stop = Arc::new(AtomicBool::new(false));
            let target = AudioTarget { pid: std::process::id(), include_tree: false };
            let (s, st, m) = (sink.clone(), stop.clone(), muted.clone());
            let run = std::thread::spawn(move || WasapiLoopback.run_muting(target, &m, s, st).unwrap());
            std::thread::sleep(Duration::from_millis(3000));
            stop.store(true, Ordering::Relaxed);
            run.join().unwrap();
            let samples = sink.0.lock().unwrap().clone();
            // pula o primeiro meio segundo (as capturas ainda enchendo)
            let tail = &samples[samples.len().min(48_000)..];
            let (p440, p660) = (power(tail, 440.0), power(tail, 660.0));
            println!("muted {muted:?}: {} ms of sound, 440 Hz {p440:.5}, 660 Hz {p660:.5}", samples.len() / 96);
            results.push((p440, p660));
        }
        // os tocadores terminam sozinhos com o fim do som; o arquivo fica até lá
        let _ = (a, b);

        assert!(apps.iter().any(|a| a.id == "powershell") && apps.iter().any(|a| a.id == "pwsh"), "{apps:?}");
        let heard = |p: f32| p > 0.002;
        assert!(heard(results[0].0) && heard(results[0].1), "nothing muted: both tones");
        assert!(heard(results[1].0) && !heard(results[1].1), "pwsh muted: only 440 Hz");
        assert!(!heard(results[2].0) && heard(results[2].1), "powershell muted: only 660 Hz");
        assert!(!heard(results[3].0) && !heard(results[3].1), "both muted: neither");
    }

    #[test]
    fn a_queue_that_grows_too_much_is_trimmed() {
        let mut mixer = Mixer::default();
        mixer.push(1, tone(0.5, MAX_QUEUE + CHUNK));
        assert_eq!(mixer.inputs[&1].queue.len(), PRIME);
    }
}
