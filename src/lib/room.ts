// Cliente da sala: conversa com o servidor de sinalização e gerencia as
// conexões WebRTC. Cada "assistir" é uma conexão própria (quem transmite ->
// quem assiste), então quem transmite só gasta upload com quem está vendo.

import type { NativeCapture } from "./capture";
import { describeError, log } from "./log";
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
  /** Tipos de rota (host/srflx/relay) achados aqui e recebidos do outro lado. */
  localRoutes: string[] = [];
  remoteRoutes: string[] = [];
  constructor(
    public pc: RTCPeerConnection,
    /** "sharing" (eu transmito) ou "watching" (eu assisto). */
    public area: "sharing" | "watching",
    /** Ex.: 'to "Ana"' ou 'from "Ana"'. */
    public who: string,
  ) {}
  run(step: string, fn: () => Promise<void>) {
    this.chain = this.chain
      .then(fn)
      .catch((e) => log.error(this.area, `${step} failed (${this.who}): ${describeError(e)}`));
  }
}

const ROUTE_NAMES: Record<string, string> = {
  host: "local network",
  srflx: "public address (STUN)",
  prflx: "public address (discovered)",
  relay: "relay (TURN)",
};

/** Ex.: "2 local network, 1 public address (STUN)". */
function summarizeRoutes(types: string[]): string {
  if (!types.length) return "none";
  const counts = new Map<string, number>();
  types.forEach((t) => counts.set(t, (counts.get(t) ?? 0) + 1));
  return [...counts].map(([t, n]) => `${n} ${ROUTE_NAMES[t] ?? t}`).join(", ");
}

/** Lê do getStats qual rota de rede foi escolhida. */
async function describeConnected(conn: Conn) {
  const stats = await conn.pc.getStats();
  let route = "unknown route";
  stats.forEach((s) => {
    if (s.type === "candidate-pair" && s.nominated && s.state === "succeeded") {
      const local = stats.get(s.localCandidateId);
      const remote = stats.get(s.remoteCandidateId);
      route =
        `${ROUTE_NAMES[local?.candidateType] ?? local?.candidateType} -> ` +
        `${ROUTE_NAMES[remote?.candidateType] ?? remote?.candidateType} over ${local?.protocol ?? "?"}`;
    }
  });
  log.info(conn.area, `Connected ${conn.who} via ${route}`);
}

/** Alguns segundos depois de conectar: o vídeo está mesmo saindo/chegando? */
async function describeVideo(conn: Conn) {
  if (conn.pc.connectionState !== "connected") return;
  const stats = await conn.pc.getStats();
  stats.forEach((s) => {
    if (s.kind !== "video") return;
    if (s.type === "outbound-rtp") {
      const msg =
        `Video ${conn.who} after 5s: ${s.framesEncoded ?? 0} frames encoded, ` +
        `${s.frameWidth ?? "?"}x${s.frameHeight ?? "?"} at ${s.framesPerSecond ?? 0} fps, ` +
        `${Math.round((s.bytesSent ?? 0) / 1024)} KB sent, encoder: ${s.encoderImplementation ?? "unknown"}, ` +
        `limited by: ${s.qualityLimitationReason ?? "nothing"}`;
      if (s.framesEncoded) log.info(conn.area, msg);
      else log.error(conn.area, `${msg}. No video is going out!`);
    }
    if (s.type === "inbound-rtp") {
      const msg =
        `Video ${conn.who} after 5s: ${s.framesDecoded ?? 0} frames decoded, ` +
        `${s.frameWidth ?? "?"}x${s.frameHeight ?? "?"} at ${s.framesPerSecond ?? 0} fps, ` +
        `${Math.round((s.bytesReceived ?? 0) / 1024)} KB received, decoder: ${s.decoderImplementation ?? "unknown"}`;
      if (s.framesDecoded) log.info(conn.area, msg);
      else log.error(conn.area, `${msg}. No video is coming in!`);
    }
  });
}

/** Liga os logs de estado de uma conexão; `onState` recebe cada mudança. */
function instrument(conn: Conn, onState?: (state: RTCPeerConnectionState) => void) {
  const { pc } = conn;
  let candidateErrors = 0;
  let lastState = "";
  pc.onicegatheringstatechange = () => {
    if (pc.iceGatheringState === "complete") {
      log.info(conn.area, `Network routes found on this PC ${conn.who}: ${summarizeRoutes(conn.localRoutes)}`);
    }
  };
  pc.onicecandidateerror = (e) => {
    const ev = e as RTCPeerConnectionIceErrorEvent;
    if (candidateErrors++ < 3) {
      log.warn(conn.area, `Could not reach ${ev.url} ${conn.who} (code ${ev.errorCode}: ${ev.errorText})`);
    }
  };
  pc.onconnectionstatechange = () => {
    const state = pc.connectionState;
    onState?.(state);
    if (state === lastState || state === "new") return;
    lastState = state;
    switch (state) {
      case "connecting":
        log.info(conn.area, `Connecting ${conn.who}...`);
        break;
      case "connected":
        describeConnected(conn).catch(() => {});
        setTimeout(() => describeVideo(conn).catch(() => {}), 5000);
        break;
      case "disconnected":
        log.warn(conn.area, `Connection ${conn.who} dropped, trying to recover`);
        break;
      case "failed":
        log.error(
          conn.area,
          `Connection ${conn.who} FAILED. Routes on this PC: ${summarizeRoutes(conn.localRoutes)}. ` +
            `Routes from the other PC: ${summarizeRoutes(conn.remoteRoutes)}`,
        );
        break;
    }
  };
}

/** Descreve as faixas que vão ser enviadas (para achar faixa morta ou vazia). */
function describeTracks(stream: MediaStream): string {
  return stream
    .getTracks()
    .map((t) => {
      const s = t.getSettings();
      const size = t.kind === "video" && s.width ? ` ${s.width}x${s.height}` : "";
      return `${t.kind}${size} (${t.readyState}${t.enabled ? "" : ", disabled"}${t.muted ? ", muted" : ""})`;
    })
    .join(", ");
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
  /** Conta as tentativas de assistir cada pessoa (para o aviso de "travou"). */
  private watchAttempt = new Map<string, number>();

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

  private nameOf(id: string) {
    const name = this.peers.get(id)?.name;
    return name ? `"${name}"` : `someone (#${id})`;
  }

  // ---------- conexão com o servidor ----------

  /** Tenta conectar por até ~45s (o DNS de túneis novos pode demorar). */
  async connect(onAttempt?: (n: number) => void): Promise<void> {
    const deadline = Date.now() + 45_000;
    const where = this.url.startsWith("ws://127.0.0.1") ? "my own room" : this.url;
    log.info("room", `Joining ${where} as "${this.name}"`);
    for (let attempt = 1; Date.now() < deadline; attempt++) {
      if (this.disposed) throw new Error("cancelado");
      onAttempt?.(attempt);
      try {
        await this.open();
        return;
      } catch (e) {
        if (e instanceof WrongPasswordError) {
          log.warn("room", "Wrong room password");
          throw e;
        }
        if (attempt === 1 || attempt % 5 === 0) {
          log.info("room", `Could not reach the room yet (attempt ${attempt}), trying again`);
        }
        await sleep(2000);
      }
    }
    log.error("room", "Gave up joining the room after 45s");
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
        let msg: ServerMsg;
        try {
          msg = JSON.parse(ev.data) as ServerMsg;
        } catch {
          log.warn("room", "Got an unreadable message from the room server");
          return;
        }
        if (msg.type === "error") {
          wrongPassword = msg.reason === "wrong-password";
          return;
        }
        try {
          this.handle(msg);
        } catch (e) {
          log.error("room", `Something broke while handling "${msg.type}": ${describeError(e)}`);
        }
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
    log.warn("room", "Lost the connection to the room server");
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
    if (this.status !== "closed") log.info("room", "Left the room");
    this.disposed = true;
    this.status = "closed";
    this.teardown();
    this.ws?.close();
    this.emit();
  }

  private handle(msg: ServerMsg) {
    switch (msg.type) {
      case "welcome": {
        this.selfId = msg.id;
        this.settings = msg.settings ?? DEFAULT_SETTINGS;
        this.peers.set(msg.id, { id: msg.id, name: this.name, sharing: false });
        msg.peers.forEach((p) => this.peers.set(p.id, p));
        this.status = "connected";
        const others = msg.peers.map((p) => `"${p.name}"`).join(", ");
        log.info("room", `Joined the room (${others ? `${others} already there` : "nobody else there yet"})`);
        break;
      }
      case "peer-joined":
        this.peers.set(msg.peer.id, msg.peer);
        log.info("room", `${this.nameOf(msg.peer.id)} joined`);
        this.onPeerEvent?.("join");
        break;
      case "peer-left":
        log.info("room", `${this.nameOf(msg.id)} left`);
        this.peers.delete(msg.id);
        this.onPeerEvent?.("leave");
        this.closeOut(msg.id);
        this.dropView(msg.id);
        break;
      case "peer-updated": {
        const p = this.peers.get(msg.id);
        if (p) {
          if (msg.id !== this.selfId && msg.sharing !== p.sharing) {
            log.info(
              "room",
              `${this.nameOf(msg.id)} ${msg.sharing ? `started sharing (${msg.label ?? "?"})` : "stopped sharing"}`,
            );
          }
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
      log.warn("sharing", "Tried to start sharing while not connected to the room");
      capture.stop();
      return;
    }
    this.stopShare();
    this.capture = capture;
    this.quality = quality;
    this.localStream = capture.stream;
    capture.onEnded = () => this.stopShare();

    const label = `${qualityLabel(quality)} · ${sourceTitle}`.slice(0, 48);
    log.info(
      "sharing",
      `Started sharing "${sourceTitle}" at ${qualityLabel(quality)}. Tracks: ${describeTracks(capture.stream)}`,
    );
    const me = this.peers.get(this.selfId)!;
    me.sharing = true;
    me.label = label;
    this.sendRaw({ type: "sharing", sharing: true, label });
    this.emit();
  }

  stopShare() {
    if (!this.localStream) return;
    log.info("sharing", "Stopped sharing");
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
    const who = `to ${this.nameOf(viewerId)}`;
    const stream = this.localStream;
    if (!stream) {
      log.warn("sharing", `${this.nameOf(viewerId)} wants to watch, but I'm not sharing anymore`);
      return;
    }
    log.info("sharing", `${this.nameOf(viewerId)} wants to watch. Tracks: ${describeTracks(stream)}`);
    this.closeOut(viewerId);

    const pc = new RTCPeerConnection(RTC_CONFIG);
    const conn = new Conn(pc, "sharing", who);
    this.out.set(viewerId, conn);
    instrument(conn);

    pc.onicecandidate = (e) => {
      if (e.candidate) {
        conn.localRoutes.push(e.candidate.type ?? "unknown");
        this.signal(viewerId, { kind: "ice", role: "sharer", candidate: e.candidate.toJSON() });
      }
    };
    try {
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
    } catch (e) {
      log.error("sharing", `Could not add the video/audio to the connection ${who}: ${describeError(e)}`);
      return;
    }
    conn.run("Creating the offer", async () => {
      const offer = await pc.createOffer();
      const codecs = new Set(offer.sdp!.match(/a=rtpmap:\d+ [\w-]+/g)?.map((l) => l.split(" ")[1]));
      await pc.setLocalDescription({ type: "offer", sdp: tuneOpus(offer.sdp!) });
      this.signal(viewerId, { kind: "offer", sdp: pc.localDescription!.sdp });
      log.info("sharing", `Sent the offer ${who} (codecs: ${[...codecs].join(", ") || "none!"})`);
    });
  }

  // ---------- assistir ----------

  watch(id: string) {
    this.watching.add(id);
    if (id !== this.selfId) {
      log.info("watching", `Asked to watch ${this.nameOf(id)}`);
      this.views.set(id, { stream: null, state: "connecting" });
      this.signal(id, { kind: "watch" });
      const attempt = (this.watchAttempt.get(id) ?? 0) + 1;
      this.watchAttempt.set(id, attempt);
      setTimeout(() => this.checkStuck(id, attempt), 20_000);
    }
    this.emit();
  }

  /** Se ficou 20s em "conectando", diz no log em que etapa parou. */
  private checkStuck(id: string, attempt: number) {
    if (this.watchAttempt.get(id) !== attempt) return; // já tentou de novo
    if (this.views.get(id)?.state !== "connecting") return;
    const conn = this.inc.get(id);
    const stage = !conn
      ? "their PC never sent an offer (the problem is on their side, check their log)"
      : `offer received, connection is "${conn.pc.connectionState}", ` +
        `routes here: ${summarizeRoutes(conn.localRoutes)}, from them: ${summarizeRoutes(conn.remoteRoutes)}`;
    log.warn("watching", `Still connecting to ${this.nameOf(id)} after 20s: ${stage}`);
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
    log.info("watching", `Retrying ${this.nameOf(id)}`);
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
    try {
      this.handleSignal(from, data);
    } catch (e) {
      log.error("room", `Something broke while handling "${data.kind}" from ${this.nameOf(from)}: ${describeError(e)}`);
    }
  }

  private handleSignal(from: string, data: SignalData) {
    switch (data.kind) {
      case "watch":
        this.startSending(from);
        break;
      case "unwatch":
        log.info("sharing", `${this.nameOf(from)} stopped watching`);
        this.closeOut(from);
        break;
      case "offer": {
        if (!this.watching.has(from)) return;
        log.info("watching", `Got the offer from ${this.nameOf(from)}`);
        this.inc.get(from)?.pc.close();
        const pc = new RTCPeerConnection(RTC_CONFIG);
        const conn = new Conn(pc, "watching", `from ${this.nameOf(from)}`);
        this.inc.set(from, conn);
        instrument(conn, (state) => {
          if (this.inc.get(from)?.pc !== pc) return;
          if (state === "connected") this.setView(from, { state: "connected" });
          if (state === "failed") this.setView(from, { state: "failed" });
        });

        pc.onicecandidate = (e) => {
          if (e.candidate) {
            conn.localRoutes.push(e.candidate.type ?? "unknown");
            this.signal(from, { kind: "ice", role: "viewer", candidate: e.candidate.toJSON() });
          }
        };
        pc.ontrack = (e) => {
          const stream = e.streams[0] ?? new MediaStream([e.track]);
          this.setView(from, { stream });
        };
        conn.run("Answering the offer", async () => {
          await pc.setRemoteDescription({ type: "offer", sdp: data.sdp });
          const answer = await pc.createAnswer();
          await pc.setLocalDescription({ type: "answer", sdp: tuneOpus(answer.sdp!) });
          this.signal(from, { kind: "answer", sdp: pc.localDescription!.sdp });
          log.info("watching", `Sent the answer to ${this.nameOf(from)}`);
        });
        break;
      }
      case "answer": {
        const conn = this.out.get(from);
        if (!conn) {
          log.warn("sharing", `Got an answer from ${this.nameOf(from)}, but no connection was waiting for it`);
          break;
        }
        log.info("sharing", `Got the answer from ${this.nameOf(from)}`);
        conn.run("Applying the answer", () => conn.pc.setRemoteDescription({ type: "answer", sdp: data.sdp }));
        break;
      }
      case "ice": {
        const conn = data.role === "sharer" ? this.inc.get(from) : this.out.get(from);
        if (!conn) break;
        const type = data.candidate.candidate?.match(/ typ (\w+)/)?.[1];
        if (type) conn.remoteRoutes.push(type);
        conn.run("Adding a network route", () => conn.pc.addIceCandidate(data.candidate));
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
