// Cliente da sala: conversa com o servidor de sinalização e gerencia as
// conexões WebRTC. Cada "assistir" é uma conexão própria (quem transmite ->
// quem assiste), então quem transmite só gasta upload com quem está vendo.

import type { NativeCapture } from "./capture";
import { describeError, log } from "./log";
import {
  DEFAULT_SETTINGS,
  HEIGHTS,
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
  | { kind: "ice"; role: "sharer" | "viewer"; candidate: RTCIceCandidateInit }
  /** Tamanho em que quem assiste está vendo a tela (altura em pixels; 0 = app escondido). */
  | { kind: "view"; height: number };

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
  /** Última qualidade aplicada para este espectador (para logar só mudanças). */
  applied = "";
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
      const codec = stats.get(s.codecId)?.mimeType?.replace("video/", "") ?? "?";
      const msg =
        `Video ${conn.who} after 5s: ${codec}, ${s.framesEncoded ?? 0} frames encoded, ` +
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

/**
 * Ordem dos codecs de vídeo. Com aceleração, H.264 primeiro: o WebView2
 * codifica na placa de vídeo (NVENC, QuickSync, AMF), quase sem custo no
 * processador. Sem aceleração, VP8 primeiro (sempre em software).
 */
function preferCodecs(transceiver: RTCRtpTransceiver, hw: boolean) {
  const codecs = RTCRtpSender.getCapabilities("video")?.codecs;
  if (!codecs || !transceiver.setCodecPreferences) return;
  const rank = (c: RTCRtpCodec) => {
    const mime = c.mimeType.toLowerCase();
    const fmtp = c.sdpFmtpLine ?? "";
    if (mime === "video/h264") {
      // perfil baseline (42e0..) com packetization-mode=1: o que todo encoder de placa aceita
      const baseline = /profile-level-id=42e0/i.test(fmtp) ? 0 : 1;
      const mode1 = /packetization-mode=1/.test(fmtp) ? 0 : 2;
      return (hw ? 0 : 10) + baseline + mode1;
    }
    if (mime === "video/vp8") return hw ? 10 : 0;
    if (mime === "video/vp9") return 20;
    if (mime === "video/av1") return 30;
    return 40; // rtx, red, ulpfec: mantém no fim
  };
  const sorted = [...codecs].sort((a, b) => rank(a) - rank(b));
  try {
    transceiver.setCodecPreferences(sorted);
  } catch (e) {
    log.warn("sharing", `Could not set the video codec order: ${describeError(e)}`);
  }
}

/** Altura (360/480/720/1080) que basta para um vídeo 16:9 exibido nesse tamanho em pixels CSS. */
function heightTier(width: number, height: number): number {
  // object-fit: contain; o que conta é a altura visível
  const visible = Math.min(height, (width * 9) / 16) * devicePixelRatio;
  return HEIGHTS.find((h) => h >= visible * 0.9) ?? HEIGHTS[HEIGHTS.length - 1];
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
  /** Prefere H.264 (codificado pela placa de vídeo) ao VP8 (no processador). */
  private hwEncode = true;
  /** Conta as tentativas de assistir cada pessoa (para o aviso de "travou"). */
  private watchAttempt = new Map<string, number>();
  /** Quem assiste: altura em que estou vendo cada pessoa, e o que já avisei a ela. */
  private viewHeight = new Map<string, number>();
  private sentView = new Map<string, number>();
  /** Quem transmite: altura pedida por cada espectador. */
  private viewerWants = new Map<string, number>();
  /** Tela aberta na janela flutuante e a altura dela (continua visível com o app minimizado). */
  private pip: { id: string; height: number } | null = null;
  private onVisibility = () => this.watching.forEach((id) => this.sendView(id));

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
    document.removeEventListener("visibilitychange", this.onVisibility);
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
        document.addEventListener("visibilitychange", this.onVisibility);
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
        this.viewerWants.delete(msg.id);
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
  startShare(capture: NativeCapture, quality: Quality, sourceTitle: string, hwEncode: boolean) {
    if (this.status !== "connected") {
      log.warn("sharing", "Tried to start sharing while not connected to the room");
      capture.stop();
      return;
    }
    this.stopShare();
    this.capture = capture;
    this.quality = quality;
    this.hwEncode = hwEncode;
    this.localStream = capture.stream;
    capture.onEnded = () => this.stopShare();

    const label = `${qualityLabel(quality)} · ${sourceTitle}`.slice(0, 48);
    log.info(
      "sharing",
      `Started sharing "${sourceTitle}" at ${qualityLabel(quality)}, ` +
        `preferring ${hwEncode ? "H.264 (GPU)" : "VP8 (CPU)"}. Tracks: ${describeTracks(capture.stream)}`,
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
        const transceiver = pc.addTransceiver(track, {
          direction: "sendonly",
          streams: [stream],
          sendEncodings: [
            track.kind === "video"
              ? { maxBitrate: videoBitrate(this.quality), maxFramerate: this.quality.fps }
              : { maxBitrate: AUDIO_BITRATE },
          ],
        });
        if (track.kind === "video") preferCodecs(transceiver, this.hwEncode);
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
    if (this.viewerWants.has(viewerId)) this.applyView(viewerId);
  }

  /**
   * Ajusta o vídeo que vai para um espectador ao tamanho em que ele está
   * vendo: miniatura recebe resolução e fps menores, e quem está com o app
   * escondido não recebe vídeo (só áudio). Poupa upload e a placa de vídeo.
   */
  private applyView(viewerId: string) {
    const conn = this.out.get(viewerId);
    if (!conn) return;
    conn.run("Adjusting the quality", async () => {
      const sender = conn.pc.getSenders().find((s) => s.track?.kind === "video");
      const params = sender?.getParameters();
      const enc = params?.encodings?.[0];
      if (!sender || !params || !enc) return;
      const wants = this.viewerWants.get(viewerId) ?? this.quality.height;
      let summary: string;
      if (wants === 0) {
        enc.active = false;
        summary = "paused my video for them (AbreTela is hidden on their PC), audio keeps going";
      } else {
        const height = Math.min(wants, this.quality.height);
        const fps = height < this.quality.height && height <= 480 ? Math.min(this.quality.fps, 30) : this.quality.fps;
        const source = this.capture?.frameHeight || this.quality.height;
        enc.active = true;
        enc.scaleResolutionDownBy = Math.max(1, source / height);
        enc.maxFramerate = fps;
        enc.maxBitrate = videoBitrate({ height, fps });
        summary = `sending ${qualityLabel({ height, fps })}`;
      }
      if (conn.applied === summary) return;
      await sender.setParameters(params);
      conn.applied = summary;
      log.info("sharing", `${this.nameOf(viewerId)}: ${summary}`);
    });
  }

  // ---------- assistir ----------

  watch(id: string) {
    this.watching.add(id);
    if (id !== this.selfId) {
      log.info("watching", `Asked to watch ${this.nameOf(id)}`);
      this.views.set(id, { stream: null, state: "connecting" });
      this.signal(id, { kind: "watch" });
      this.sentView.delete(id);
      this.sendView(id);
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

  /** O tile de alguém mudou de tamanho (em pixels CSS). */
  setViewSize(id: string, width: number, height: number) {
    this.viewHeight.set(id, heightTier(width, height));
    this.sendView(id);
  }

  /** A janela flutuante abriu (com o tamanho dela), mudou de tamanho ou fechou (`null`). */
  setPip(id: string | null, width = 0, height = 0) {
    const previous = this.pip?.id;
    this.pip = id ? { id, height: heightTier(width, height) } : null;
    if (previous && previous !== id) this.sendView(previous);
    if (id) this.sendView(id);
  }

  /** Avisa quem transmite o tamanho em que estou vendo (só quando muda). */
  private sendView(id: string) {
    if (id === this.selfId || !this.watching.has(id)) return;
    const tile = this.viewHeight.get(id);
    const pip = this.pip?.id === id ? this.pip.height : 0;
    if (tile === undefined && !pip) return;
    // a janela flutuante continua visível mesmo com o app minimizado
    const height = Math.max(document.hidden ? 0 : (tile ?? 0), pip);
    if (this.sentView.get(id) === height) return;
    this.sentView.set(id, height);
    this.signal(id, { kind: "view", height });
  }

  private dropView(id: string) {
    this.sentView.delete(id);
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
        this.viewerWants.delete(from);
        this.closeOut(from);
        break;
      case "view":
        this.viewerWants.set(from, Math.max(0, Number(data.height) || 0));
        this.applyView(from);
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

/**
 * Link de convite: a URL pública da sala (que abre a página do convite) com a
 * senha depois do "#" (o navegador nunca envia essa parte ao servidor).
 */
export const makeInvite = (roomUrl: string, password: string) => `${roomUrl}/#${encodeURIComponent(password)}`;

export type Invite = {
  /** URL https pública da sala, sem barra no fim. */
  room: string;
  /** Endereço do WebSocket de sinalização. */
  ws: string;
  password: string | null;
};

/** Nome curto para mostrar a sala (o primeiro pedaço do endereço). */
export const roomLabel = (roomUrl: string) => {
  try {
    return new URL(roomUrl).hostname.split(".")[0];
  } catch {
    return roomUrl;
  }
};

/**
 * Lê um convite. Aceita o link (https://endereço/#senha), o mesmo link vindo
 * pelo protocolo (abretela://join/endereço#senha), só o endereço (com ou sem
 * "#senha") e, por compatibilidade, os convites antigos que eram só o código
 * do túnel do trycloudflare ("abc-def#senha").
 */
export function parseInvite(input: string): Invite | null {
  const raw = input.trim().replace(/^abretela:\/\/join\//i, "https://");
  const hashAt = raw.indexOf("#");
  let password = hashAt >= 0 ? raw.slice(hashAt + 1).trim() : "";
  try {
    password = decodeURIComponent(password);
  } catch {}
  let base = (hashAt >= 0 ? raw.slice(0, hashAt) : raw).trim();
  if (!base) return null;

  if (/^[a-z0-9-]+$/i.test(base)) {
    base = `https://${base.toLowerCase()}.trycloudflare.com`; // convite antigo
  } else if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(base)) {
    base = `https://${base}`;
  }
  let url: URL;
  try {
    url = new URL(base.replace(/^ws(s?):/i, "http$1:"));
  } catch {
    return null;
  }
  if (!/^https?:$/.test(url.protocol) || (!url.hostname.includes(".") && url.hostname !== "localhost")) return null;
  const path = url.pathname.replace(/\/+$/, "").replace(/\/ws$/, "");
  const room = `${url.protocol}//${url.host}${path}`;
  return { room, ws: `${room.replace(/^http/, "ws")}/ws`, password: password || null };
}
