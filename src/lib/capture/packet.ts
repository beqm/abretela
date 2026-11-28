// Formato dos pacotes que a captura nativa (Rust, capture/packet.rs) manda
// pelo WebSocket local. Os dois lados conferem os mesmos exemplos em
// packet-fixture.json.
//
// Cabeçalho de 16 bytes: [tipo, formato, largura u16 LE, altura u16 LE, 0...],
// seguido dos dados: vídeo em I420 ou NV12 ou áudio em f32 estéreo a 48 kHz.
//
// A mídia recebida de outra pessoa pela recepção nativa (Rust, receive/) vem
// ainda codificada (H.264 em Annex B ou Opus), com outro cabeçalho de 16
// bytes: [tipo, marcas, 0 x 6, tempo em microssegundos u64 LE].

export const HEADER = 16;
export const KIND_VIDEO = 1;
export const KIND_AUDIO = 2;
export const FORMAT_I420 = 0;
export const FORMAT_NV12 = 1;
/** Vídeo recebido, ainda codificado (H.264, uma unidade de acesso em Annex B). */
export const KIND_ENCODED_VIDEO = 3;
/** Som recebido, ainda codificado (um pacote Opus). */
export const KIND_ENCODED_AUDIO = 4;
/** Marca (segundo byte) de quadro-chave. */
export const FLAG_KEYFRAME = 1;

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

export type EncodedHeader = { keyframe: boolean; timestampUs: number };

/** Lê o cabeçalho de um pacote de mídia recebida ainda codificada. */
export function readEncodedHeader(buf: ArrayBuffer): EncodedHeader {
  const dv = new DataView(buf);
  return {
    keyframe: (dv.getUint8(1) & FLAG_KEYFRAME) !== 0,
    timestampUs: Number(dv.getBigUint64(8, true)),
  };
}
