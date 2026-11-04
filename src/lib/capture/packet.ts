// Formato dos pacotes que a captura nativa (Rust, capture/packet.rs) manda
// pelo WebSocket local. Os dois lados conferem os mesmos exemplos em
// packet-fixture.json.
//
// Cabeçalho de 16 bytes: [tipo, formato, largura u16 LE, altura u16 LE, 0...],
// seguido dos dados: vídeo em I420 ou NV12 ou áudio em f32 estéreo a 48 kHz.

export const HEADER = 16;
export const KIND_VIDEO = 1;
export const KIND_AUDIO = 2;
export const FORMAT_I420 = 0;
export const FORMAT_NV12 = 1;

export type VideoHeader = { format: "I420" | "NV12"; width: number; height: number };

/** Tipo do pacote (KIND_VIDEO ou KIND_AUDIO). */
export const packetKind = (buf: ArrayBuffer) => new Uint8Array(buf, 0, 1)[0];

/** Lê o cabeçalho de um pacote de vídeo. */
export function readVideoHeader(buf: ArrayBuffer): VideoHeader {
  const dv = new DataView(buf);
  return {
    format: dv.getUint8(1) === FORMAT_NV12 ? "NV12" : "I420",
    width: dv.getUint16(2, true),
    height: dv.getUint16(4, true),
  };
}

/** Os pixels (vídeo) ou amostras (áudio) depois do cabeçalho. */
export const payload = (buf: ArrayBuffer) => new Uint8Array(buf, HEADER);
