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
