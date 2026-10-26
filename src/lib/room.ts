// Cliente da sala: conversa com o servidor de sinalização e gerencia as
// conexões WebRTC. Cada "assistir" é uma conexão própria (quem transmite ->
// quem assiste), então quem transmite só gasta upload com quem está vendo.

import type { NativeCapture } from "./capture";
import {
  DEFAULT_SETTINGS,
  qualityLabel,
  videoBitrate,
  type Quality,
  type RoomSettings,
} from "./quality";

export type PeerInfo = { id: string; name: string; sharing: boolean; label?: string | null };

export type RemoteView = {
  stream: MediaStream | null;
  state: "connecting" | "connected" | "failed";
};

type SignalData =
  | { kind: "watch" }
  | { kind: "unwatch" }
  | { kind: "offer"; sdp: string }
  | { kind: "answer"; sdp: string }
  | { kind: "ice"; role: "sharer" | "viewer"; candidate: RTCIceCandidateInit };

type ServerMsg =
  | { type: "welcome"; id: string; peers: PeerInfo[]; settings: RoomSettings }
  | { type: "peer-joined"; peer: PeerInfo }
  | { type: "peer-left"; id: string }
  | { type: "peer-updated"; id: string; sharing: boolean; label?: string | null }
  | { type: "signal"; from: string; data: SignalData }
  | { type: "error"; reason: string };

export class WrongPasswordError extends Error {
  constructor() {
    super("Senha incorreta.");
  }
}

const RTC_CONFIG: RTCConfiguration = {
  iceServers: [
    { urls: ["stun:stun.cloudflare.com:3478", "stun:stun.l.google.com:19302"] },
  ],
};

const AUDIO_BITRATE = 128_000;

/** Executa as operações de uma conexão em ordem (evita corridas de SDP/ICE). */
class Conn {
  private chain: Promise<void> = Promise.resolve();
  constructor(public pc: RTCPeerConnection) {}
  run(fn: () => Promise<void>) {
    this.chain = this.chain.then(fn).catch((e) => console.error("[rtc]", e));
  }
}

/** Liga áudio estéreo e com bitrate alto no Opus (o padrão é voz mono). */
function tuneOpus(sdp: string): string {
  const pt = sdp.match(/a=rtpmap:(\d+) opus\/48000\/2/i)?.[1];
  if (!pt) return sdp;
  const extra = `stereo=1;sprop-stereo=1;maxaveragebitrate=${AUDIO_BITRATE}`;
  const fmtp = new RegExp(`a=fmtp:${pt} ([^\\r\\n]*)`);
  if (fmtp.test(sdp)) {
    return sdp.replace(fmtp, (line, params: string) =>
      params.includes("stereo=1") ? line : `a=fmtp:${pt} ${params};${extra}`,
    );
  }
  return sdp.replace(
    new RegExp(`(a=rtpmap:${pt} [^\\r\\n]*\\r\\n)`),
    `$1a=fmtp:${pt} ${extra}\r\n`,
  );
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export class RoomClient {
  selfId = "";
  peers = new Map<string, PeerInfo>();
  localStream: MediaStream | null = null;
  settings: RoomSettings = DEFAULT_SETTINGS;
  /** Volume/mudo por pessoa (sobrevive à troca de layout). */
  volumes = new Map<string, { volume: number; muted: boolean }>();
  /** Avisos para efeitos sonoros. */
  onPeerEvent: ((kind: "join" | "leave" | "live") => void) | null = null;
  /** Quem eu escolhi assistir (pode incluir meu próprio id = prévia). */
  watching = new Set<string>();
  views = new Map<string, RemoteView>();
  status: "connecting" | "connected" | "closed" = "connecting";
  version = 0;

  private ws: WebSocket | null = null;
  private out = new Map<string, Conn>(); // eu transmito -> viewer
  private inc = new Map<string, Conn>(); // sharer -> eu assisto
  private listeners = new Set<() => void>();
  private disposed = false;
  private capture: NativeCapture | null = null;
  private quality: Quality = { height: 720, fps: 30 };

  constructor(
    private url: string,
    private name: string,
    private password: string,
  ) {}

  subscribe = (fn: () => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };
  getVersion = () => this.version;

  private emit() {
    this.version++;
    this.listeners.forEach((f) => f());
  }

  // ---------- conexão com o servidor ----------

  /** Tenta conectar por até ~45s (o DNS de túneis novos pode demorar). */
  async connect(onAttempt?: (n: number) => void): Promise<void> {
    const deadline = Date.now() + 45_000;
    for (let attempt = 1; Date.now() < deadline; attempt++) {
      if (this.disposed) throw new Error("cancelado");
      onAttempt?.(attempt);
      try {
        await this.open();
        return;
      } catch (e) {
        if (e instanceof WrongPasswordError) throw e;
        await sleep(2000);
      }
    }
    throw new Error("connect-failed");
  }

  private open(): Promise<void> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(this.url);
      let wrongPassword = false;
      ws.onopen = () => {
        this.ws = ws;
        this.sendRaw({ type: "join", name: this.name, password: this.password });
      };
      ws.onmessage = (ev) => {
        const msg = JSON.parse(ev.data) as ServerMsg;
        if (msg.type === "error") {
          wrongPassword = msg.reason === "wrong-password";
          return;
        }
        this.handle(msg);
        if (msg.type === "welcome") resolve();
      };
      ws.onclose = () => {
        if (!this.selfId) reject(wrongPassword ? new WrongPasswordError() : new Error("fechado"));
        else this.onClosed();
      };
    });
  }

  private sendRaw(msg: unknown) {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(msg));
  }

  private signal(to: string, data: SignalData) {
    this.sendRaw({ type: "signal", to, data });
  }

  private onClosed() {
    if (this.status === "closed") return;
    this.status = "closed";
    this.teardown();
    this.emit();
  }

  private teardown() {
    this.capture?.stop();
    this.capture = null;
    this.localStream = null;
    this.out.forEach((c) => c.pc.close());
    this.inc.forEach((c) => c.pc.close());
    this.out.clear();
    this.inc.clear();
    this.views.clear();
    this.watching.clear();
  }

  leave() {
    this.disposed = true;
    this.status = "closed";
    this.teardown();
    this.ws?.close();
    this.emit();
  }

  private handle(msg: ServerMsg) {
    switch (msg.type) {
      case "welcome":
        this.selfId = msg.id;
        this.settings = msg.settings ?? DEFAULT_SETTINGS;
        this.peers.set(msg.id, { id: msg.id, name: this.name, sharing: false });
        msg.peers.forEach((p) => this.peers.set(p.id, p));
        this.status = "connected";
        break;
      case "peer-joined":
        this.peers.set(msg.peer.id, msg.peer);
        this.onPeerEvent?.("join");
        break;
      case "peer-left":
        this.peers.delete(msg.id);
        this.onPeerEvent?.("leave");
        this.closeOut(msg.id);
        this.dropView(msg.id);
        break;
      case "peer-updated": {
        const p = this.peers.get(msg.id);
        if (p) {
          if (msg.sharing && !p.sharing && msg.id !== this.selfId) this.onPeerEvent?.("live");
          p.sharing = msg.sharing;
          p.label = msg.label;
        }
        if (!msg.sharing && msg.id !== this.selfId) this.dropView(msg.id);
        break;
      }
      case "signal":
        this.onSignal(msg.from, msg.data);
        break;
    }
    this.emit();
  }

  // ---------- transmitir ----------

  /** Começa a transmitir uma captura nativa já iniciada. */
  startShare(capture: NativeCapture, quality: Quality, sourceTitle: string) {
    if (this.status !== "connected") {
      capture.stop();
      return;
    }
    this.stopShare();
    this.capture = capture;
    this.quality = quality;
    this.localStream = capture.stream;
    capture.onEnded = () => this.stopShare();

    const label = `${qualityLabel(quality)} · ${sourceTitle}`.slice(0, 48);
    const me = this.peers.get(this.selfId)!;
    me.sharing = true;
    me.label = label;
    this.sendRaw({ type: "sharing", sharing: true, label });
    this.emit();
  }

  stopShare() {
    if (!this.localStream) return;
    this.capture?.stop();
    this.capture = null;
    this.localStream = null;
    this.out.forEach((c) => c.pc.close());
    this.out.clear();
    this.watching.delete(this.selfId);
    const me = this.peers.get(this.selfId);
    if (me) {
      me.sharing = false;
      me.label = null;
    }
    this.sendRaw({ type: "sharing", sharing: false });
    this.emit();
  }

  private closeOut(viewerId: string) {
    this.out.get(viewerId)?.pc.close();
    this.out.delete(viewerId);
  }

  private startSending(viewerId: string) {
    const stream = this.localStream;
    if (!stream) return;
    this.closeOut(viewerId);

    const pc = new RTCPeerConnection(RTC_CONFIG);
    const conn = new Conn(pc);
    this.out.set(viewerId, conn);

    pc.onicecandidate = (e) => {
      if (e.candidate)
        this.signal(viewerId, { kind: "ice", role: "sharer", candidate: e.candidate.toJSON() });
    };
    for (const track of stream.getTracks()) {
      pc.addTransceiver(track, {
        direction: "sendonly",
        streams: [stream],
        sendEncodings: [
          track.kind === "video"
            ? { maxBitrate: videoBitrate(this.quality), maxFramerate: this.quality.fps }
            : { maxBitrate: AUDIO_BITRATE },
        ],
      });
    }
    conn.run(async () => {
      const offer = await pc.createOffer();
      await pc.setLocalDescription({ type: "offer", sdp: tuneOpus(offer.sdp!) });
      this.signal(viewerId, { kind: "offer", sdp: pc.localDescription!.sdp });
    });
  }

  // ---------- assistir ----------

  watch(id: string) {
    this.watching.add(id);
    if (id !== this.selfId) {
      this.views.set(id, { stream: null, state: "connecting" });
      this.signal(id, { kind: "watch" });
    }
    this.emit();
  }

  unwatch(id: string) {
    if (id !== this.selfId && this.watching.has(id)) this.signal(id, { kind: "unwatch" });
    this.dropView(id);
    this.emit();
  }

  watchAll() {
    for (const p of this.peers.values()) {
      if (p.sharing && p.id !== this.selfId && !this.watching.has(p.id)) this.watch(p.id);
    }
  }

  retry(id: string) {
    this.unwatch(id);
    this.watch(id);
  }

  private dropView(id: string) {
    this.watching.delete(id);
    this.views.delete(id);
    this.inc.get(id)?.pc.close();
    this.inc.delete(id);
  }

  private setView(id: string, patch: Partial<RemoteView>) {
    const v = this.views.get(id);
    if (!v) return;
    this.views.set(id, { ...v, ...patch });
    this.emit();
  }

  private onSignal(from: string, data: SignalData) {
    switch (data.kind) {
      case "watch":
        this.startSending(from);
        break;
      case "unwatch":
        this.closeOut(from);
        break;
      case "offer": {
        if (!this.watching.has(from)) return;
        this.inc.get(from)?.pc.close();
        const pc = new RTCPeerConnection(RTC_CONFIG);
        const conn = new Conn(pc);
        this.inc.set(from, conn);

        pc.onicecandidate = (e) => {
          if (e.candidate)
            this.signal(from, { kind: "ice", role: "viewer", candidate: e.candidate.toJSON() });
        };
        pc.ontrack = (e) => {
          const stream = e.streams[0] ?? new MediaStream([e.track]);
          this.setView(from, { stream });
        };
        pc.onconnectionstatechange = () => {
          if (this.inc.get(from)?.pc !== pc) return;
          if (pc.connectionState === "connected") this.setView(from, { state: "connected" });
          if (pc.connectionState === "failed") this.setView(from, { state: "failed" });
        };
        conn.run(async () => {
          await pc.setRemoteDescription({ type: "offer", sdp: data.sdp });
          const answer = await pc.createAnswer();
          await pc.setLocalDescription({ type: "answer", sdp: tuneOpus(answer.sdp!) });
          this.signal(from, { kind: "answer", sdp: pc.localDescription!.sdp });
        });
        break;
      }
      case "answer": {
        const conn = this.out.get(from);
        conn?.run(() => conn.pc.setRemoteDescription({ type: "answer", sdp: data.sdp }));
        break;
      }
      case "ice": {
        const conn = data.role === "sharer" ? this.inc.get(from) : this.out.get(from);
        conn?.run(() => conn.pc.addIceCandidate(data.candidate));
        break;
      }
    }
  }
}

/** Convite no formato "codigo#senha". */
export const makeInvite = (code: string, password: string) => `${code}#${password}`;

/**
 * Lê um convite. Aceita "abc-def", "abc-def#senha", "abc-def.trycloudflare.com"
 * ou a URL completa.
 */
export function parseInvite(
  input: string,
): { url: string; code: string; password: string | null } | null {
  const [rawCode, ...rest] = input.trim().split("#");
  const code = rawCode
    .trim()
    .toLowerCase()
    .replace(/^(https?|wss?):\/\//, "")
    .split("/")[0]
    .replace(/\.trycloudflare\.com$/, "");
  if (!/^[a-z0-9-]+$/.test(code)) return null;
  const password = rest.length ? rest.join("#").trim() : null;
  return { url: `wss://${code}.trycloudflare.com/ws`, code, password: password || null };
}
