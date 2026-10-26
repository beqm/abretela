// Recebe os frames da captura nativa (Rust) por um WebSocket local e os
// transforma em MediaStreamTracks: vídeo via MediaStreamTrackGenerator e
// áudio via AudioWorklet -> MediaStreamAudioDestinationNode.

import { invoke } from "@tauri-apps/api/core";

declare class MediaStreamTrackGenerator extends MediaStreamTrack {
  constructor(init: { kind: "video" | "audio" });
  readonly writable: WritableStream<VideoFrame>;
}

export type Source = {
  id: string;
  kind: "screen" | "window";
  title: string;
  app: string;
};

export type NativeCapture = {
  stream: MediaStream;
  stop: () => void;
  /** Chamado quando a captura termina sozinha (ex.: janela fechada). */
  onEnded: (() => void) | null;
};

const PKT_VIDEO = 1;
const PKT_AUDIO = 2;
const HEADER = 16;

export const listSources = () => invoke<Source[]>("list_sources");
export const getThumbnail = (id: string) => invoke<string | null>("thumbnail", { id });

export async function startNativeCapture(
  sourceId: string,
  height: number,
  fps: number,
  audio: boolean,
): Promise<NativeCapture> {
  const url = await invoke<string>("start_capture", { id: sourceId, height, fps, audio });
  const token = url.split("/").pop()!;

  const video = new MediaStreamTrackGenerator({ kind: "video" });
  video.contentHint = "motion";
  const writer = video.writable.getWriter();
  const tracks: MediaStreamTrack[] = [video];

  let audioCtx: AudioContext | null = null;
  let player: AudioWorkletNode | null = null;
  if (audio) {
    audioCtx = new AudioContext({ sampleRate: 48000, latencyHint: "interactive" });
    await audioCtx.audioWorklet.addModule("/audio-worklet.js");
    player = new AudioWorkletNode(audioCtx, "pcm-player", {
      numberOfInputs: 0,
      outputChannelCount: [2],
    });
    const dest = audioCtx.createMediaStreamDestination();
    dest.channelCount = 2;
    player.connect(dest);
    await audioCtx.resume();
    tracks.push(dest.stream.getAudioTracks()[0]);
  }

  const ws = new WebSocket(url);
  ws.binaryType = "arraybuffer";
  let pending = 0;
  let stopped = false;

  const capture: NativeCapture = { stream: new MediaStream(tracks), stop, onEnded: null };

  ws.onmessage = (ev) => {
    const buf = ev.data as ArrayBuffer;
    const kind = new Uint8Array(buf, 0, 1)[0];
    if (kind === PKT_VIDEO) {
      if (pending > 1) return; // encoder atrasado: descarta
      const dv = new DataView(buf);
      const frame = new VideoFrame(new Uint8Array(buf, HEADER), {
        format: "I420",
        codedWidth: dv.getUint16(2, true),
        codedHeight: dv.getUint16(4, true),
        timestamp: Math.round(performance.now() * 1000),
        colorSpace: { primaries: "bt709", transfer: "bt709", matrix: "bt709", fullRange: false },
      });
      pending++;
      writer
        .write(frame)
        .catch(() => frame.close())
        .finally(() => pending--);
    } else if (kind === PKT_AUDIO && player) {
      player.port.postMessage(new Float32Array(buf, HEADER), [buf]);
    }
  };
  ws.onclose = () => {
    if (stopped) return;
    stop();
    capture.onEnded?.();
  };

  function stop() {
    if (stopped) return;
    stopped = true;
    ws.close();
    writer.close().catch(() => {});
    tracks.forEach((t) => t.stop());
    audioCtx?.close();
    invoke("stop_capture", { token });
  }

  return capture;
}
