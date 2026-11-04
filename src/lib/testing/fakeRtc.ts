// WebRTC falso para testes: guarda o que o código pediu (faixas, SDP,
// parâmetros de envio) sem rede nem navegador.

import { vi } from "vitest";

export type FakeTrack = {
  kind: string;
  readyState: string;
  enabled: boolean;
  muted: boolean;
  getSettings: () => object;
};

export const track = (kind: string): FakeTrack => ({
  kind,
  readyState: "live",
  enabled: true,
  muted: false,
  getSettings: () => ({}),
});

export const stream = (...kinds: string[]) => {
  const tracks = kinds.map(track);
  return { getTracks: () => tracks, getVideoTracks: () => tracks.filter((t) => t.kind === "video") } as unknown as MediaStream;
};

export class FakeSender {
  params: { encodings: RTCRtpEncodingParameters[] };
  constructor(
    public track: FakeTrack,
    encodings: RTCRtpEncodingParameters[],
  ) {
    this.params = { encodings: encodings.map((e) => ({ ...e })) };
  }
  getParameters() {
    return this.params;
  }
  async setParameters(p: { encodings: RTCRtpEncodingParameters[] }) {
    this.params = p;
  }
  async replaceTrack(t: FakeTrack) {
    this.track = t;
  }
}

export class FakePC {
  static all: FakePC[] = [];
  closed = false;
  connectionState = "new";
  senders: FakeSender[] = [];
  localDescription: RTCSessionDescriptionInit | null = null;
  remoteDescription: RTCSessionDescriptionInit | null = null;
  ontrack: ((e: { streams: MediaStream[]; track: FakeTrack }) => void) | null = null;
  onicecandidate: unknown = null;
  onconnectionstatechange: (() => void) | null = null;
  constructor() {
    FakePC.all.push(this);
  }
  addTransceiver(t: FakeTrack, init: { sendEncodings: RTCRtpEncodingParameters[] }) {
    this.senders.push(new FakeSender(t, init.sendEncodings));
    return { setCodecPreferences() {} };
  }
  getSenders() {
    return this.senders;
  }
  async createOffer() {
    return { type: "offer", sdp: "v=0\r\na=rtpmap:96 VP8/90000\r\n" };
  }
  async createAnswer() {
    return { type: "answer", sdp: "v=0\r\n" };
  }
  async setLocalDescription(d: RTCSessionDescriptionInit) {
    this.localDescription = d;
  }
  async setRemoteDescription(d: RTCSessionDescriptionInit) {
    this.remoteDescription = d;
  }
  async addIceCandidate() {}
  async getStats() {
    return new Map();
  }
  close() {
    this.closed = true;
  }
  /** Simula a conexão mudando de estado. */
  setState(state: RTCPeerConnectionState) {
    this.connectionState = state;
    this.onconnectionstatechange?.();
  }
}

/** Troca RTCPeerConnection e companhia pelos falsos (desfazer com `vi.unstubAllGlobals`). */
export function installFakeRtc() {
  FakePC.all = [];
  vi.stubGlobal("RTCPeerConnection", FakePC);
  vi.stubGlobal("RTCRtpSender", { getCapabilities: () => ({ codecs: [] }) });
  vi.stubGlobal("MediaStream", class {});
}

/** Deixa as promessas e os setTimeout(0) pendentes andarem. */
export async function settle(rounds = 20) {
  for (let i = 0; i < rounds; i++) await new Promise((r) => setTimeout(r, 0));
}
