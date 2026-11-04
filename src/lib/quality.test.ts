import { describe, expect, it } from "vitest";
import {
  DEFAULT_SETTINGS,
  allowedFps,
  allowedHeights,
  clampQuality,
  encodingFor,
  heightTier,
  rankVideoCodec,
  videoBitrate,
  viewerHeight,
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

describe("viewerHeight", () => {
  it("pede o tamanho do tile", () => {
    expect(viewerHeight(720, 0, false)).toBe(720);
  });

  it("com o app escondido pede 0 (só áudio)", () => {
    expect(viewerHeight(720, 0, true)).toBe(0);
  });

  it("a janela flutuante continua valendo com o app escondido", () => {
    expect(viewerHeight(720, 480, true)).toBe(480);
    expect(viewerHeight(360, 720, false)).toBe(720);
  });

  it("sem tamanho ainda, não pede nada", () => {
    expect(viewerHeight(undefined, 0, false)).toBeNull();
  });
});

describe("encodingFor", () => {
  const q = { height: 1080, fps: 60 };

  it("0 desliga o vídeo", () => {
    expect(encodingFor(0, q, 1080)).toEqual({ active: false });
  });

  it("nunca manda mais que a qualidade escolhida", () => {
    expect(encodingFor(2160, q, 1080)).toMatchObject({ height: 1080, fps: 60, scaleDownBy: 1 });
  });

  it("miniatura recebe menos resolução e no máximo 30 fps", () => {
    expect(encodingFor(360, q, 1080)).toMatchObject({ height: 360, fps: 30, scaleDownBy: 3, maxBitrate: 800_000 });
  });

  it("720p de uma fonte 1080p mantém o fps", () => {
    expect(encodingFor(720, q, 1080)).toMatchObject({ height: 720, fps: 60, scaleDownBy: 1.5 });
  });

  it("não aumenta uma fonte menor que o pedido", () => {
    expect(encodingFor(720, q, 600)).toMatchObject({ scaleDownBy: 1 });
  });
});

describe("rankVideoCodec", () => {
  const h264 = (fmtp: string) => ({ mimeType: "video/H264", sdpFmtpLine: fmtp });
  const order = (hw: boolean, codecs: { mimeType: string; sdpFmtpLine?: string }[]) =>
    [...codecs].sort((a, b) => rankVideoCodec(a, hw) - rankVideoCodec(b, hw)).map((c) => c.mimeType + (c.sdpFmtpLine ?? ""));
  const all = [
    { mimeType: "video/rtx" },
    { mimeType: "video/VP9" },
    h264("profile-level-id=640c1f;packetization-mode=1"),
    { mimeType: "video/VP8" },
    h264("profile-level-id=42e01f;packetization-mode=0"),
    h264("profile-level-id=42e01f;packetization-mode=1"),
    { mimeType: "video/AV1" },
  ];

  it("com aceleração, H.264 baseline modo 1 primeiro", () => {
    expect(order(true, all)).toEqual([
      "video/H264profile-level-id=42e01f;packetization-mode=1",
      "video/H264profile-level-id=640c1f;packetization-mode=1",
      "video/H264profile-level-id=42e01f;packetization-mode=0",
      "video/VP8",
      "video/VP9",
      "video/AV1",
      "video/rtx",
    ]);
  });

  it("sem aceleração, VP8 primeiro", () => {
    expect(order(false, all)[0]).toBe("video/VP8");
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
