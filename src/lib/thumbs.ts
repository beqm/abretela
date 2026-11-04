// Prévias das telas: uma imagem pequena de quem está ao vivo, pedida ao
// passar o mouse na pessoa. Vai pela sinalização da sala, sem abrir conexão
// de vídeo.

/** Mensagens das prévias entre as pessoas. */
export type ThumbSignal = { kind: "thumb-request" } | { kind: "thumb"; image: string | null };

export type Thumb = { image: string | null; at: number };

/** Por quanto tempo a prévia de alguém vale antes de pedir outra. */
const THUMB_TTL = 5 * 60_000;
/** Sem resposta nesse tempo, desiste (app antigo ou travado). */
const THUMB_TIMEOUT = 5000;
/** A prévia da minha tela é reaproveitada por esse tempo. */
const OWN_THUMB_TTL = 30_000;

/** Uma imagem JPEG pequena do vídeo de `stream` (para a prévia), ou null. */
export async function snapshot(stream: MediaStream, width = 320): Promise<string | null> {
  const track = stream.getVideoTracks()[0];
  if (!track || track.readyState !== "live") return null;
  const video = document.createElement("video");
  video.muted = true;
  video.srcObject = new MediaStream([track]);
  try {
    await video.play();
    if (!video.videoWidth) await new Promise((r) => video.addEventListener("loadeddata", r, { once: true }));
    const height = Math.round((width * video.videoHeight) / video.videoWidth) || Math.round((width * 9) / 16);
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    canvas.getContext("2d")!.drawImage(video, 0, 0, width, height);
    return canvas.toDataURL("image/jpeg", 0.7);
  } catch {
    return null;
  } finally {
    video.pause();
    video.srcObject = null;
  }
}

/** Aceita só uma imagem JPEG de tamanho razoável vinda de outra pessoa. */
export const isValidThumb = (image: unknown): image is string =>
  typeof image === "string" && image.startsWith("data:image/jpeg") && image.length < 300_000;

export class Thumbs {
  /** Prévias recebidas, por pessoa. */
  readonly images = new Map<string, Thumb>();
  private pending = new Set<string>();
  /** A prévia da minha tela, reaproveitada quando várias pessoas pedem juntas. */
  private own: Thumb | null = null;

  constructor(
    private signal: (to: string, data: ThumbSignal) => void,
    private isSharing: (id: string) => boolean,
    private changed: () => void,
  ) {}

  /** Pede a prévia da tela de alguém, se a guardada tiver mais de 5 minutos. */
  request(id: string) {
    const cached = this.images.get(id);
    if (cached && Date.now() - cached.at < THUMB_TTL) return;
    if (this.pending.has(id) || !this.isSharing(id)) return;
    this.pending.add(id);
    this.signal(id, { kind: "thumb-request" });
    this.changed();
    setTimeout(() => {
      if (!this.pending.delete(id)) return;
      // sem resposta (app antigo ou travado): tenta de novo daqui a 1 minuto
      this.images.set(id, { image: null, at: Date.now() - THUMB_TTL + 60_000 });
      this.changed();
    }, THUMB_TIMEOUT);
  }

  isLoading(id: string) {
    return this.pending.has(id);
  }

  /** A prévia de alguém não vale mais (trocou de tela, parou ou saiu). */
  forget(id: string) {
    this.images.delete(id);
  }

  /** Minha tela mudou: a próxima prévia pedida é tirada de novo. */
  resetOwn() {
    this.own = null;
  }

  /** Trata uma mensagem de prévia; `stream` é o que estou transmitindo. */
  handleSignal(from: string, data: ThumbSignal, stream: MediaStream | null) {
    if (data.kind === "thumb-request") {
      void this.sendOwn(from, stream);
      return;
    }
    this.pending.delete(from);
    this.images.set(from, { image: isValidThumb(data.image) ? data.image : null, at: Date.now() });
    this.changed();
  }

  private async sendOwn(to: string, stream: MediaStream | null) {
    if (!stream) return;
    if (!this.own || Date.now() - this.own.at > OWN_THUMB_TTL) {
      this.own = { image: await snapshot(stream), at: Date.now() };
    }
    this.signal(to, { kind: "thumb", image: this.own.image });
  }
}
