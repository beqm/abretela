import { describe, expect, it } from "vitest";
import {
  DEFAULT_SETTINGS,
  allowedFps,
  allowedHeights,
  clampQuality,
  heightTier,
  videoBitrate,
  type RoomSettings,
} from "./quality";

describe("heightTier", () => {
  it("escolhe a altura que cobre o vídeo visível", () => {
    expect(heightTier(640, 360, 1)).toBe(360);
    expect(heightTier(1280, 720, 1)).toBe(720);
    expect(heightTier(1920, 1080, 1)).toBe(1080);
  });

  it("não passa da maior altura", () => {
    expect(heightTier(3840, 2160, 1)).toBe(1080);
  });

  it("usa a altura visível com object-fit: contain", () => {
    // tile alto e estreito: o vídeo 16:9 ocupa só 225px de altura
    expect(heightTier(400, 1000, 1)).toBe(360);
  });

  it("tem 10% de folga antes de subir de nível", () => {
    expect(heightTier(1000, 400, 1)).toBe(360);
    expect(heightTier(1000, 401, 1)).toBe(480);
  });

  it("conta a escala da tela (devicePixelRatio)", () => {
    expect(heightTier(640, 360, 2)).toBe(720);
  });
});

describe("clampQuality", () => {
  const narrow: RoomSettings = { minHeight: 480, maxHeight: 720, minFps: 30, maxFps: 30 };

  it("mantém o que já está dentro dos limites", () => {
    expect(clampQuality({ height: 720, fps: 30 }, DEFAULT_SETTINGS)).toEqual({ height: 720, fps: 30 });
  });

  it("puxa para o valor permitido mais próximo", () => {
    expect(clampQuality({ height: 1080, fps: 60 }, narrow)).toEqual({ height: 720, fps: 30 });
    expect(clampQuality({ height: 360, fps: 15 }, narrow)).toEqual({ height: 480, fps: 30 });
  });

  it("no empate, fica com o menor", () => {
    expect(clampQuality({ height: 600, fps: 30 }, DEFAULT_SETTINGS).height).toBe(480);
  });
});

describe("limites da sala", () => {
  it("lista só as opções permitidas", () => {
    const s: RoomSettings = { minHeight: 480, maxHeight: 720, minFps: 15, maxFps: 30 };
    expect(allowedHeights(s)).toEqual([480, 720]);
    expect(allowedFps(s)).toEqual([15, 30]);
    expect(allowedHeights(DEFAULT_SETTINGS)).toEqual([360, 480, 720, 1080]);
  });
});

describe("videoBitrate", () => {
  it("ajusta pela resolução e pelo fps", () => {
    expect(videoBitrate({ height: 720, fps: 30 })).toBe(2_500_000);
    expect(videoBitrate({ height: 1080, fps: 60 })).toBe(7_200_000);
    expect(videoBitrate({ height: 360, fps: 15 })).toBe(560_000);
  });

  it("altura desconhecida usa o valor de 720p", () => {
    expect(videoBitrate({ height: 900, fps: 30 })).toBe(2_500_000);
  });
});
