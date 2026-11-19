// Recebe os frames da captura nativa (Rust) por um WebSocket local e os
// transforma em MediaStreamTracks: vídeo via MediaStreamTrackGenerator (ou,
// onde ele não existe, um canvas) e áudio via AudioWorklet ->
// MediaStreamAudioDestinationNode.

import { invoke } from "@tauri-apps/api/core";
import { describeError, log } from "../log";
import { HEADER, KIND_AUDIO, KIND_VIDEO, packetKind, payload, readVideoHeader } from "./packet";
import { canSendFromWebView } from "./support";

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

/** Onde os frames viram uma faixa de vídeo. `write` fica com o frame. */
type VideoOutput = {
  track: MediaStreamTrack;
  write: (frame: VideoFrame) => Promise<void>;
  close: () => void;
};

/** No Chromium (WebView2): a faixa recebe os frames direto e serve para o WebRTC. */
function generatorOutput(): VideoOutput {
  const track = new MediaStreamTrackGenerator({ kind: "video" });
  track.contentHint = "motion";
  const writer = track.writable.getWriter();
  return {
    track,
    write: (frame) => writer.write(frame),
    close: () => void writer.close().catch(() => {}),
  };
}

/**
 * Sem MediaStreamTrackGenerator (WKWebView, macOS): cada frame é desenhado num
 * canvas, e a faixa é o vídeo desse canvas. É só a minha prévia: ali quem
 * manda a tela é o envio nativo (`support.ts`).
 */
function canvasOutput(): VideoOutput {
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d", { alpha: false })!;
  const track = canvas.captureStream().getVideoTracks()[0];
  return {
    track,
    write: async (frame) => {
      try {
        if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
          canvas.width = frame.displayWidth;
          canvas.height = frame.displayHeight;
        }
        ctx.drawImage(frame, 0, 0);
      } finally {
        frame.close();
      }
    },
    close: () => {},
  };
}

export const listSources = () => invoke<Source[]>("list_sources");
export const getThumbnail = (id: string) => invoke<string | null>("thumbnail", { id });
/**
 * O sistema deixa o app capturar a tela? (No Windows, sempre.) Com `ask`, pede
 * a permissão: o macOS pergunta na primeira vez e, depois, abre os Ajustes.
 */
export const capturePermission = (ask = false) => invoke<boolean>("capture_permission", { ask });

export async function startNativeCapture(
  sourceId: string,
  height: number,
  fps: number,
  audio: boolean,
  gpu: boolean,
  /** Bitrate máximo do envio nativo (experimental); sem ele, só o caminho de sempre. */
  nativeSend?: number,
): Promise<NativeCapture> {
  const url = await invoke<string>("start_capture", { id: sourceId, height, fps, audio, gpu, nativeSend });
  const token = url.split("/").pop()!;

  const video = canSendFromWebView() ? generatorOutput() : canvasOutput();
  const tracks: MediaStreamTrack[] = [video.track];

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
      video
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
    video.close();
    tracks.forEach((t) => t.stop());
    audioCtx?.close();
    invoke("stop_capture", { token });
  }

  return capture;
}
