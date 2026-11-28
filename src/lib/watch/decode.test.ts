import { describe, expect, it } from "vitest";
import { codecFromAnnexB, interleaveStereo, KeyframeGate, s16ToF32 } from "./decode";

const bytes = (...values: number[]) => new Uint8Array(values);

describe("codec do H.264 recebido", () => {
  it("vem do SPS do quadro-chave (perfil, restrições e nível)", () => {
    // SPS (tipo 7) de Constrained Baseline 3.1, depois PPS e IDR
    const frame = bytes(0, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f, 0xaa, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65, 0x88);
    expect(codecFromAnnexB(frame)).toBe("avc1.42e01f");
  });

  it("acha o SPS depois de outros NALs e com código de início de 3 bytes", () => {
    // delimitador (tipo 9), depois o SPS de High 4.0
    const frame = bytes(0, 0, 0, 1, 0x09, 0xf0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28, 0xac);
    expect(codecFromAnnexB(frame)).toBe("avc1.640028");
  });

  it("não inventa quando o frame não traz SPS ou ele vem cortado", () => {
    expect(codecFromAnnexB(bytes(0, 0, 0, 1, 0x41, 0x9a, 0x02))).toBeNull();
    expect(codecFromAnnexB(bytes(0, 0, 0, 1, 0x67, 0x42))).toBeNull();
    expect(codecFromAnnexB(bytes())).toBeNull();
  });
});

describe("por onde o vídeo pode começar", () => {
  it("espera o primeiro quadro-chave", () => {
    const gate = new KeyframeGate();
    expect(gate.accepts(false)).toBe(false);
    expect(gate.accepts(true)).toBe(true);
    expect(gate.accepts(false)).toBe(true);
  });

  it("depois de perder um frame, só volta num quadro-chave", () => {
    const gate = new KeyframeGate();
    gate.accepts(true);
    gate.lost();
    expect(gate.accepts(false)).toBe(false);
    expect(gate.accepts(false)).toBe(false);
    expect(gate.accepts(true)).toBe(true);
    expect(gate.accepts(false)).toBe(true);
  });
});

describe("som decodificado para o tocador (estéreo intercalado)", () => {
  it("intercala um plano por canal", () => {
    const stereo = interleaveStereo([new Float32Array([0.1, 0.2]), new Float32Array([-0.1, -0.2])]);
    expect(Array.from(stereo)).toEqual([0.1, -0.1, 0.2, -0.2].map(Math.fround));
  });

  it("repete o mono nos dois lados", () => {
    expect(Array.from(interleaveStereo([new Float32Array([0.5, 0.25])]))).toEqual([0.5, 0.5, 0.25, 0.25]);
    expect(interleaveStereo([]).length).toBe(0);
  });

  it("converte inteiros de 16 bits para a faixa de -1 a 1", () => {
    expect(Array.from(s16ToF32(new Int16Array([0, 16384, -32768, 32767])))).toEqual(
      [0, 0.5, -1, 32767 / 32768].map(Math.fround),
    );
  });
});
