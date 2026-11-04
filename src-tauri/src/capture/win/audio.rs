//! Som de um processo (janela) ou de tudo menos este app (tela inteira), com o
//! WASAPI process loopback.

use crate::capture::contracts::{AudioSource, AudioTarget, BoxError, FrameSink};
use crate::capture::packet::audio_packet;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

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
}
