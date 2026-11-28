//! Som do sistema no Linux, pelo PipeWire: o que está saindo na saída de som
//! padrão (o "monitor" dela), em estéreo a 48 kHz. O PipeWire já entrega nesse
//! formato, convertendo se a saída estiver em outro; aqui o som só é cortado
//! nos pedaços de 10 ms que o resto do app espera.
//!
//! O som é o da saída inteira: inclui o do próprio AbreTela. Quem transmite
//! com som e assiste alguém ao mesmo tempo devolve a essa pessoa o som dela.
//! No Windows e no macOS o sistema deixa tirar o próprio app; aqui, só
//! ligando o som de cada aplicativo à captura, um por um, o que fica para
//! depois.

use super::run_until_stopped;
use crate::capture::{
    contracts::{AudioSource, AudioTarget, BoxError, FrameSink},
    packet::audio_packet,
};
use pipewire::{self as pw, properties::properties, spa};
use std::sync::{atomic::AtomicBool, Arc};

/// 10 ms de f32 estéreo a 48 kHz, o pedaço que o resto do app espera.
const CHUNK: usize = 480 * 8;
const RATE: u32 = 48_000;

/// Corta o som que chega em pedaços inteiros de 10 ms, guardando a sobra para
/// juntar com o que vier depois.
#[derive(Default)]
struct Chunker {
    pending: Vec<u8>,
}

impl Chunker {
    fn push(&mut self, bytes: &[u8], mut emit: impl FnMut(&[u8])) {
        self.pending.extend_from_slice(bytes);
        let whole = self.pending.len() / CHUNK * CHUNK;
        for chunk in self.pending[..whole].chunks_exact(CHUNK) {
            emit(chunk);
        }
        self.pending.drain(..whole);
    }
}

/// O formato pedido ao PipeWire: f32 intercalado, estéreo, 48 kHz.
fn wanted_format() -> Result<Vec<u8>, String> {
    use spa::param::audio::{AudioFormat, AudioInfoRaw};
    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::F32LE);
    info.set_rate(RATE);
    info.set_channels(2);
    let mut position = [0; spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &spa::pod::Value::Object(object))
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(|e| format!("could not describe the audio format: {e:?}"))
}

pub struct PipewireAudio;

impl AudioSource for PipewireAudio {
    fn run(&self, _target: AudioTarget, sink: Arc<dyn FrameSink>, stop: Arc<AtomicBool>) -> Result<(), BoxError> {
        let failed = |what: &'static str| move |e: pw::Error| format!("PipeWire: {what}: {e}");
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(failed("could not start"))?;
        let context = pw::context::ContextBox::new(mainloop.loop_(), None).map_err(failed("could not start"))?;
        // o PipeWire da sessão (o do portal só mostra a tela escolhida)
        let core = context.connect(None).map_err(failed("could not connect"))?;
        let stream = pw::stream::StreamBox::new(
            &core,
            "abretela-audio",
            properties! {
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Music",
                // o que sai na saída de som padrão, em vez de um microfone
                *pw::keys::STREAM_CAPTURE_SINK => "true",
            },
        )
        .map_err(failed("could not create the stream"))?;

        let out = sink.clone();
        let _listener = stream
            .add_local_listener_with_user_data(Chunker::default())
            .state_changed(|_, _, _, new| {
                if let pw::stream::StreamState::Error(e) = new {
                    crate::log_error!("audio", "The audio stream failed: {e}");
                }
            })
            .process(move |stream, chunker| {
                let Some(mut buffer) = stream.dequeue_buffer() else { return };
                let Some(data) = buffer.datas_mut().first_mut() else { return };
                let (size, offset) = (data.chunk().size() as usize, data.chunk().offset() as usize);
                let Some(bytes) = data.data().and_then(|d| d.get(offset..offset + size)) else { return };
                chunker.push(bytes, |chunk| out.audio(audio_packet(chunk.iter().copied())));
            })
            .register()
            .map_err(failed("could not listen to the stream"))?;

        let format = wanted_format()?;
        let mut params = [spa::pod::Pod::from_bytes(&format).ok_or("PipeWire: the audio format came out invalid")?];
        stream
            .connect(
                spa::utils::Direction::Input,
                None,
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut params,
            )
            .map_err(failed("could not connect the stream"))?;
        crate::log_info!("audio", "Capturing the audio of the whole system (what plays on the default output)");

        run_until_stopped(&mainloop, &stop, &sink)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::packet::VideoPacket;
    use axum::body::Bytes;
    use std::{
        sync::{atomic::Ordering, Mutex},
        thread,
        time::Duration,
    };

    #[test]
    fn sound_comes_out_in_whole_10_ms_pieces_whatever_size_it_comes_in() {
        let sound: Vec<u8> = (0..CHUNK * 3 + 100).map(|i| i as u8).collect();
        let mut chunker = Chunker::default();
        let mut out = Vec::new();
        // pedaços de tamanhos quebrados, como o sistema manda
        for piece in sound.chunks(1000) {
            chunker.push(piece, |chunk| out.push(chunk.to_vec()));
        }
        assert_eq!(out.len(), 3, "three whole pieces; the last 100 bytes wait for more");
        assert!(out.iter().all(|c| c.len() == CHUNK));
        assert_eq!(out.concat(), sound[..CHUNK * 3], "nothing lost, repeated or out of order");
        chunker.push(&vec![7; CHUNK - 100], |chunk| out.push(chunk.to_vec()));
        assert_eq!(out.len(), 4, "the leftover completes with what comes next");
        assert_eq!(out[3][..100], sound[CHUNK * 3..]);
    }

    struct Collect(Mutex<Vec<Bytes>>);

    impl FrameSink for Collect {
        fn video(&self, _: VideoPacket) {}
        fn audio(&self, packet: Bytes) {
            self.0.lock().unwrap().push(packet);
        }
        fn is_closed(&self) -> bool {
            false
        }
    }

    /// Captura 2 s do som do sistema. Com um tom tocando na saída padrão:
    ///
    /// ```sh
    /// gst-launch-1.0 audiotestsrc freq=440 volume=0.5 ! pipewiresink &
    /// cargo test system_audio -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn system_audio() {
        use crate::capture::packet::HEADER;
        let sink = Arc::new(Collect(Mutex::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (out, flag) = (sink.clone(), stop.clone());
        let capture = thread::spawn(move || PipewireAudio.run(AudioTarget { pid: 0, include_tree: false }, out, flag));
        thread::sleep(Duration::from_secs(2));
        stop.store(true, Ordering::Relaxed);
        capture.join().unwrap().expect("the capture ran");

        let packets = sink.0.lock().unwrap();
        let samples: Vec<f32> = packets
            .iter()
            .flat_map(|p| p[HEADER..].chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect::<Vec<_>>())
            .collect();
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        let peak = left.iter().fold(0f32, |m, s| m.max(s.abs()));
        // a frequência, pelas vezes que o som cruza o zero subindo
        let crossings = left.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        let seconds = left.len() as f32 / RATE as f32;
        println!(
            "{} packets of 10 ms ({seconds:.2} s of sound), peak {peak:.2}, about {:.0} Hz",
            packets.len(),
            crossings as f32 / seconds
        );
        assert!(packets.iter().all(|p| p.len() == HEADER + CHUNK), "every packet is 10 ms of stereo f32");
        // conectar leva um tempo, e parar também: perto de 2 s, não exatamente
        assert!((120..=240).contains(&packets.len()), "close to 2 s of sound: {}", packets.len());
    }
}
