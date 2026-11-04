// Recebe os frames da captura nativa (Rust) por um WebSocket local e os
// transforma em MediaStreamTracks: vídeo via MediaStreamTrackGenerator e
// áudio via AudioWorklet -> MediaStreamAudioDestinationNode.

import { invoke } from "@tauri-apps/api/core";
import { describeError, log } from "../log";
import { HEADER, KIND_AUDIO, KIND_VIDEO, packetKind, payload, readVideoHeader } from "./packet";

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
  /** Altura real do último frame (pode ser menor que a pedida, ex.: janela pequena). */
  frameHeight: number;
};


export const listSources = () => invoke<Source[]>("list_sources");
export const getThumbnail = (id: string) => invoke<string | null>("thumbnail", { id });

export async function startNativeCapture(
  sourceId: string,
  height: number,
  fps: number,
  audio: boolean,
  gpu: boolean,
): Promise<NativeCapture> {
  const url = await invoke<string>("start_capture", { id: sourceId, height, fps, audio, gpu });
  const token = url.split("/").pop()!;

  const video = new MediaStreamTrackGenerator({ kind: "video" });
  video.contentHint = "motion";
  const writer = video.writable.getWriter();
  const tracks: MediaStreamTrack[] = [video];

  let audioCtx: AudioContext | null = null;
  let player: AudioWorkletNode | null = null;
  if (typeof MediaStreamTrackGenerator === "undefined") {
    log.error("capture", "This WebView2 has no MediaStreamTrackGenerator, video can't be sent");
  }
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
  let framesIn = 0;
  let writeErrors = 0;
  ws.onopen = () => log.info("capture", "Receiving frames from the native capture");
  ws.onerror = () => log.error("capture", "The connection to the native capture failed");

  const capture: NativeCapture = { stream: new MediaStream(tracks), stop, onEnded: null, frameHeight: 0 };

  ws.onmessage = (ev) => {
    const buf = ev.data as ArrayBuffer;
    const kind = packetKind(buf);
    if (kind === KIND_VIDEO) {
      if (pending > 1) return; // encoder atrasado: descarta
      const header = readVideoHeader(buf);
      let frame: VideoFrame;
      try {
        frame = new VideoFrame(payload(buf), {
          format: header.format,
          codedWidth: header.width,
          codedHeight: header.height,
          timestamp: Math.round(performance.now() * 1000),
          colorSpace: { primaries: "bt709", transfer: "bt709", matrix: "bt709", fullRange: false },
        });
      } catch (e) {
        if (writeErrors++ < 3) log.error("capture", `Could not create a video frame: ${describeError(e)}`);
        return;
      }
      capture.frameHeight = frame.codedHeight;
      if (++framesIn === 1) {
        log.info("capture", `First video frame reached the app (${frame.codedWidth}x${frame.codedHeight})`);
      }
      pending++;
      writer
        .write(frame)
        .catch((e) => {
          frame.close();
          if (writeErrors++ < 3) log.error("capture", `Could not hand a video frame to WebRTC: ${describeError(e)}`);
        })
        .finally(() => pending--);
    } else if (kind === KIND_AUDIO && player) {
      player.port.postMessage(new Float32Array(buf, HEADER), [buf]);
    }
  };
  ws.onclose = () => {
    if (stopped) return;
    log.warn("capture", `The native capture ended on its own after ${framesIn} frames`);
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
