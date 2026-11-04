export const HEIGHTS = [360, 480, 720, 1080] as const;
export const FPS_OPTIONS = [15, 30, 60] as const;

/** Limites de qualidade definidos por quem criou a sala. */
export type RoomSettings = {
  minHeight: number;
  maxHeight: number;
  minFps: number;
  maxFps: number;
};

export type Quality = { height: number; fps: number };

export const DEFAULT_SETTINGS: RoomSettings = {
  minHeight: 360,
  maxHeight: 1080,
  minFps: 15,
  maxFps: 60,
};

const BASE_BITRATE: Record<number, number> = {
  360: 800_000,
  480: 1_200_000,
  720: 2_500_000,
  1080: 4_500_000,
};

export function videoBitrate({ height, fps }: Quality): number {
  const base = BASE_BITRATE[height] ?? 2_500_000;
  const factor = fps >= 60 ? 1.6 : fps <= 15 ? 0.7 : 1;
  return Math.round(base * factor);
}

export const qualityLabel = ({ height, fps }: Quality) => `${height}p ${fps}fps`;

export const allowedHeights = (s: RoomSettings) =>
  HEIGHTS.filter((h) => h >= s.minHeight && h <= s.maxHeight);

export const allowedFps = (s: RoomSettings) =>
  FPS_OPTIONS.filter((f) => f >= s.minFps && f <= s.maxFps);

/**
 * Menor altura da lista que dá conta de um vídeo mostrado em `width` x
 * `height` pixels da tela (com 10% de folga).
 */
export function heightTier(width: number, height: number, dpr = globalThis.devicePixelRatio ?? 1): number {
  // object-fit: contain; o que conta é a altura visível
  const visible = Math.min(height, (width * 9) / 16) * dpr;
  return HEIGHTS.find((h) => h >= visible * 0.9) ?? HEIGHTS[HEIGHTS.length - 1];
}

/**
 * Quem assiste: altura a pedir para quem transmite. Vale o maior entre o tile
 * (0 com o app escondido) e a janela flutuante, que continua visível com o
 * app minimizado. `null` = ainda não há tamanho para pedir.
 */
export function viewerHeight(tile: number | undefined, pip: number, appHidden: boolean): number | null {
  if (tile === undefined && !pip) return null;
  return Math.max(appHidden ? 0 : (tile ?? 0), pip);
}

export type VideoEncoding =
  | { active: false }
  | { active: true; height: number; fps: number; scaleDownBy: number; maxBitrate: number };

/**
 * Quem transmite: como codificar o vídeo para um espectador que pediu
 * `wants` de altura (0 = app escondido: sem vídeo, só áudio). Miniatura
 * recebe resolução e fps menores. Poupa upload e a placa de vídeo.
 */
export function encodingFor(wants: number, quality: Quality, sourceHeight: number): VideoEncoding {
  if (wants === 0) return { active: false };
  const height = Math.min(wants, quality.height);
  const fps = height < quality.height && height <= 480 ? Math.min(quality.fps, 30) : quality.fps;
  return {
    active: true,
    height,
    fps,
    scaleDownBy: Math.max(1, sourceHeight / height),
    maxBitrate: videoBitrate({ height, fps }),
  };
}

/**
 * Ordem dos codecs de vídeo (menor = preferido). Com aceleração, H.264
 * primeiro: o WebView2 codifica na placa de vídeo (NVENC, QuickSync, AMF),
 * quase sem custo no processador. Sem aceleração, VP8 primeiro (sempre em
 * software).
 */
export function rankVideoCodec(codec: { mimeType: string; sdpFmtpLine?: string }, hw: boolean): number {
  const mime = codec.mimeType.toLowerCase();
  const fmtp = codec.sdpFmtpLine ?? "";
  if (mime === "video/h264") {
    // perfil baseline (42e0..) com packetization-mode=1: o que todo encoder de placa aceita
    const baseline = /profile-level-id=42e0/i.test(fmtp) ? 0 : 1;
    const mode1 = /packetization-mode=1/.test(fmtp) ? 0 : 2;
    return (hw ? 0 : 10) + baseline + mode1;
  }
  if (mime === "video/vp8") return hw ? 10 : 0;
  if (mime === "video/vp9") return 20;
  if (mime === "video/av1") return 30;
  return 40; // rtx, red, ulpfec: mantém no fim
}

/** Garante que uma qualidade esteja dentro dos limites da sala. */
export function clampQuality(q: Quality, s: RoomSettings): Quality {
  const heights = allowedHeights(s);
  const fps = allowedFps(s);
  const pick = (opts: readonly number[], v: number) =>
    opts.includes(v) ? v : opts.reduce((a, b) => (Math.abs(b - v) < Math.abs(a - v) ? b : a));
  return { height: pick(heights, q.height), fps: pick(fps, q.fps) };
}

export function loadJson<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? { ...fallback, ...JSON.parse(raw) } : fallback;
  } catch {
    return fallback;
  }
}

export function saveJson(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {}
}
