import { describe, expect, it } from "vitest";
import fixture from "./packet-fixture.json";
import {
  FORMAT_I420,
  FORMAT_NV12,
  HEADER,
  KIND_AUDIO,
  KIND_VIDEO,
  packetKind,
  payload,
  readVideoHeader,
} from "./packet";

/** Monta o pacote do exemplo: o cabeçalho do fixture + dados zerados. */
function build(headerHex: string, size: number): ArrayBuffer {
  const bytes = new Uint8Array(HEADER + size);
  headerHex.match(/../g)!.forEach((h, i) => (bytes[i] = parseInt(h, 16)));
  return bytes.buffer;
}

describe("pacotes da captura nativa", () => {
  it("usa as mesmas constantes do Rust", () => {
    expect(fixture.header).toBe(HEADER);
    expect(fixture.kind).toEqual({ video: KIND_VIDEO, audio: KIND_AUDIO });
    expect(fixture.format).toEqual({ i420: FORMAT_I420, nv12: FORMAT_NV12 });
  });

  it.each(fixture.examples)("lê o exemplo $name", (ex) => {
    const buf = build(ex.header_hex, ex.payload);
    expect(packetKind(buf)).toBe(ex.kind);
    expect(payload(buf).byteLength).toBe(ex.payload);
    if (ex.kind === KIND_VIDEO) {
      expect(readVideoHeader(buf)).toEqual({
        format: ex.format === FORMAT_NV12 ? "NV12" : "I420",
        width: ex.width,
        height: ex.height,
      });
      // vídeo: 1,5 byte por pixel
      expect(ex.payload).toBe((ex.width! * ex.height! * 3) / 2);
    }
  });
});
