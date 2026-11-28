// Transforma a mídia recebida pela recepção nativa (Rust, receive/), que chega
// ainda codificada pelo WebSocket local, numa MediaStream igual à que o
// WebRTC do WebView entregaria: o WebCodecs decodifica o H.264 e o Opus, o
// vídeo é desenhado num canvas e o som vai para o mesmo tocador da captura
// (`/audio-worklet.js`).
//
// Serve para os sistemas em que o WebView não tem WebRTC (Linux, WebKitGTK).
// Lá o WebCodecs depende dos codecs do sistema (GStreamer): sem eles, o
// primeiro frame já falha e `onUnsupported` avisa.

import { describeError, log } from "../log";
import {
  KIND_ENCODED_AUDIO,
  KIND_ENCODED_VIDEO,
  packetKind,
  payload,
  readEncodedHeader,
} from "../capture/packet";
import { openPlayer } from "./sound";

/** Constrained Baseline 3.1: o que o app manda quando o frame não traz o SPS. */
const DEFAULT_CODEC = "avc1.42e01f";
/** Frames esperando o decodificador; acima disso ele está atrasado. */
const MAX_QUEUE = 30;

// ---------- regras (sem navegador, para os testes) ----------

/**
 * O codec (`avc1.PPCCLL`) de um frame H.264 em Annex B, lido do SPS (NAL tipo
 * 7): perfil, restrições e nível. `null` se o frame não traz SPS.
 */
export function codecFromAnnexB(data: Uint8Array): string | null {
  for (let i = 0; i + 3 < data.length; i++) {
    if (data[i] !== 0 || data[i + 1] !== 0 || data[i + 2] !== 1) continue;
    const nal = i + 3;
    if ((data[nal] & 0x1f) !== 7) continue;
    if (nal + 3 >= data.length) return null;
    const hex = (b: number) => b.toString(16).padStart(2, "0");
    return `avc1.${hex(data[nal + 1])}${hex(data[nal + 2])}${hex(data[nal + 3])}`;
  }
  return null;
}

/**
 * Vídeo comprimido só começa por um quadro-chave, e só volta por um depois
 * de perder um frame (os seguintes dependem dele).
 */
export class KeyframeGate {
  private waiting = true;

  /** Este frame pode ir para o decodificador? */
  accepts(keyframe: boolean): boolean {
    if (keyframe) this.waiting = false;
    return !this.waiting;
  }

  /** Um frame se perdeu (erro, decodificador atrasado): espera outro quadro-chave. */
  lost() {
    this.waiting = true;
  }
}

/** Junta um plano por canal em estéreo intercalado (L, R, L, R...); mono vai para os dois lados. */
export function interleaveStereo(channels: Float32Array[]): Float32Array {
  if (!channels.length) return new Float32Array(0);
  const left = channels[0];
  const right = channels[1] ?? left;
  const out = new Float32Array(left.length * 2);
  for (let i = 0; i < left.length; i++) {
    out[i * 2] = left[i];
    out[i * 2 + 1] = right[i];
  }
  return out;
}

/** Amostras inteiras de 16 bits para a faixa de -1 a 1. */
export function s16ToF32(samples: Int16Array): Float32Array {
  const out = new Float32Array(samples.length);
  for (let i = 0; i < samples.length; i++) out[i] = samples[i] / 32768;
  return out;
}

// ---------- WebCodecs ----------

export type DecodedStream = {
  /** O vídeo (do canvas). O som não vem aqui: toca direto na saída (`sound.ts`). */
  stream: MediaStream;
  /** O volume do som desta tela, de 0 (mudo) a 1. */
  setVolume: (volume: number) => void;
  /** Um pacote do WebSocket da recepção nativa. */
  push: (buf: ArrayBuffer) => void;
  close: () => void;
  /** Avisado uma vez se este WebView não consegue decodificar (faltam codecs no sistema). */
  onUnsupported: (() => void) | null;
  /** Para os logs: frames mostrados e descartados, e o tamanho do último. */
  stats: () => { decoded: number; dropped: number; width: number; height: number };
};

/** Este WebView tem como decodificar o que a recepção nativa entrega? */
export const canDecodeInWebView = () =>
  typeof VideoDecoder !== "undefined" && typeof AudioDecoder !== "undefined";

/** As amostras de um pedaço de som decodificado, em estéreo intercalado. */
function stereoSamples(data: AudioData): Float32Array {
  const frames = data.numberOfFrames;
  const channels = Math.min(data.numberOfChannels, 2);
  const format = data.format ?? "f32-planar";
  const planar = format.endsWith("-planar");
  const integer = format.startsWith("s16");
  const plane = (index: number, length: number) => {
    if (integer) {
      const raw = new Int16Array(length);
      data.copyTo(raw, { planeIndex: index });
      return s16ToF32(raw);
    }
    const raw = new Float32Array(length);
    data.copyTo(raw, { planeIndex: index });
    return raw;
  };
  if (planar) return interleaveStereo(Array.from({ length: channels }, (_, c) => plane(c, frames)));
  const all = plane(0, frames * data.numberOfChannels);
  if (data.numberOfChannels === 2) return all;
  // intercalado com outro número de canais: fica com os dois primeiros
  const step = data.numberOfChannels;
  const pick = (channel: number) => Float32Array.from({ length: frames }, (_, i) => all[i * step + channel]);
  return interleaveStereo(Array.from({ length: channels }, (_, c) => pick(c)));
}

export async function createDecodedStream(): Promise<DecodedStream> {
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d", { alpha: false })!;
  const tracks: MediaStreamTrack[] = [canvas.captureStream().getVideoTracks()[0]];

  // som: o tocador da captura nativa, direto na saída (`sound.ts` diz por quê)
  const player = await openPlayer();
  let toldSilent = false;

  let closed = false;
  let decoded = 0;
  let dropped = 0;
  let failures = 0;
  let told = false;
  const gate = new KeyframeGate();
  let decoder: VideoDecoder | null = null;
  let codec: string | null = null;

  const result: DecodedStream = {
    stream: new MediaStream(tracks),
    setVolume: player.setVolume,
    push,
    close,
    onUnsupported: null,
    stats: () => ({ decoded, dropped, width: canvas.width, height: canvas.height }),
  };

  function unsupported(why: string) {
    if (told) return;
    told = true;
    log.error("watching", `This WebView can't decode the video (${why}). The system is probably missing its video codecs`);
    result.onUnsupported?.();
  }

  function failed(e: unknown) {
    failures++;
    if (failures <= 3) log.warn("watching", `The video decoder failed: ${describeError(e)}`);
    try {
      if (decoder && decoder.state !== "closed") decoder.close();
    } catch {}
    decoder = null;
    gate.lost();
    // nunca mostrou nada e já falhou mais de uma vez: não é um frame ruim, é o codec que falta
    if (decoded === 0 && failures >= 2) unsupported(describeError(e));
  }

  function openDecoder(next: string) {
    codec = next;
    decoder = new VideoDecoder({
      output: (frame) => {
        try {
          if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
            canvas.width = frame.displayWidth;
            canvas.height = frame.displayHeight;
          }
          ctx.drawImage(frame, 0, 0);
          if (++decoded === 1) {
            log.info("watching", `First video frame decoded in the WebView (${frame.displayWidth}x${frame.displayHeight}, ${next})`);
          }
        } finally {
          frame.close();
        }
      },
      error: failed,
    });
    decoder.configure({ codec: next, optimizeForLatency: true });
  }

  let audioFailures = 0;
  let audioDecoder = openAudioDecoder();

  function openAudioDecoder() {
    const opened = new AudioDecoder({
      output: (data) => {
        try {
          player.play(stereoSamples(data));
          if (!toldSilent && !player.audible()) {
            toldSilent = true;
            log.warn("watching", "The sound is arriving but this WebView is holding it until someone clicks in the window");
          }
        } finally {
          data.close();
        }
      },
      // um erro fecha o decodificador; o próximo pacote abre outro (`audio`)
      error: (e) => {
        if (++audioFailures <= 3) log.warn("watching", `The audio decoder failed: ${describeError(e)}`);
      },
    });
    opened.configure({ codec: "opus", sampleRate: 48000, numberOfChannels: 2 });
    return opened;
  }

  function video(buf: ArrayBuffer) {
    const { keyframe, timestampUs } = readEncodedHeader(buf);
    const data = payload(buf);
    if (!gate.accepts(keyframe)) {
      dropped++;
      return;
    }
    if (keyframe) {
      // o tamanho ou o perfil pode ter mudado (outra camada de qualidade)
      const next = codecFromAnnexB(data) ?? codec ?? DEFAULT_CODEC;
      if (!decoder || next !== codec) {
        try {
          if (decoder && decoder.state !== "closed") decoder.close();
          openDecoder(next);
        } catch (e) {
          failed(e);
          return;
        }
      }
    }
    if (!decoder) return;
    if (decoder.decodeQueueSize > MAX_QUEUE) {
      gate.lost();
      dropped++;
      return;
    }
    try {
      decoder.decode(new EncodedVideoChunk({ type: keyframe ? "key" : "delta", timestamp: timestampUs, data }));
    } catch (e) {
      failed(e);
    }
  }

  function audio(buf: ArrayBuffer) {
    if (audioDecoder.state === "closed") {
      // um pacote ruim não pode calar o som até o fim: cada pacote Opus se
      // decodifica sozinho, então dá para recomeçar do seguinte
      try {
        audioDecoder = openAudioDecoder();
      } catch (e) {
        if (++audioFailures <= 3) log.warn("watching", `Could not reopen the audio decoder: ${describeError(e)}`);
        return;
      }
    }
    if (audioDecoder.state !== "configured") return;
    const { timestampUs } = readEncodedHeader(buf);
    try {
      audioDecoder.decode(new EncodedAudioChunk({ type: "key", timestamp: timestampUs, data: payload(buf) }));
    } catch (e) {
      if (++audioFailures <= 3) log.warn("watching", `Could not decode an audio packet: ${describeError(e)}`);
    }
  }

  function push(buf: ArrayBuffer) {
    if (closed) return;
    const kind = packetKind(buf);
    if (kind === KIND_ENCODED_VIDEO) video(buf);
    else if (kind === KIND_ENCODED_AUDIO) audio(buf);
  }

  function close() {
    if (closed) return;
    closed = true;
    try {
      if (decoder && decoder.state !== "closed") decoder.close();
      if (audioDecoder.state !== "closed") audioDecoder.close();
    } catch {}
    tracks.forEach((t) => t.stop());
    player.close();
  }

  return result;
}
