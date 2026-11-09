// Cliente da sala: junta as peças (sinalização, transporte de mídia, prévias
// e política de qualidade) e expõe o estado da sala para a interface.

import type { NativeCapture } from "./capture/native";
import { describeError, log, logNetwork } from "./log";
import { DEFAULT_SETTINGS, heightTier, qualityLabel, viewerHeight, type Quality, type RoomSettings } from "./quality";
import { Signaling, type PeerInfo, type ServerMsg, type SignalData } from "./signaling";
import { Thumbs } from "./thumbs";
import { describeTracks } from "./transport/diagnostics";
import { MeshTransport } from "./transport/mesh";
import { NativeTransport } from "./transport/native";
import type { MediaTransport, Publication, RemoteView, TransportHost } from "./transport/types";

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

  private signaling: Signaling;
  private transport: MediaTransport;
  private thumbCache: Thumbs;
  private listeners = new Set<() => void>();
  private capture: NativeCapture | null = null;
  private quality: Quality = { height: 720, fps: 30 };
  /** Prefere H.264 (codificado pela placa de vídeo) ao VP8 (no processador). */
  private hwEncode = true;
  /** Conta as tentativas de assistir cada pessoa (para o aviso de "travou"). */
  private watchAttempt = new Map<string, number>();
  /** Quem assiste: altura em que estou vendo cada pessoa. */
  private viewHeight = new Map<string, number>();
  /** Tela aberta na janela flutuante e a altura dela (continua visível com o app minimizado). */
  private pip: { id: string; height: number } | null = null;
  private onVisibility = () => this.watching.forEach((id) => this.sendView(id));

  constructor(
    url: string,
    private name: string,
    password: string,
    /** Envio nativo (experimental): minha tela sai codificada pelo próprio app. */
    options: { nativeSend?: boolean } = {},
  ) {
    this.signaling = new Signaling(url, name, password);
    this.signaling.onMessage = (msg) => this.handle(msg);
    this.signaling.onClosed = () => this.onClosed();
    const host: TransportHost = {
      signal: (to, data) => this.signaling.signal(to, data),
      nameOf: (id) => this.nameOf(id),
      onRemote: (id, patch) => this.setView(id, patch),
    };
    this.nativeSend = !!options.nativeSend;
    this.transport = this.nativeSend ? new NativeTransport(host) : new MeshTransport(host);
    this.thumbCache = new Thumbs(
      (to, data) => this.signaling.signal(to, data),
      (id) => !!this.peers.get(id)?.sharing,
      () => this.emit(),
    );
  }

  /** Esta sala usa o envio nativo (escolhido ao entrar; mudar a opção vale na próxima sala). */
  readonly nativeSend: boolean;

  /** Prévias das telas de quem está ao vivo (pedidas ao passar o mouse). */
  get thumbs() {
    return this.thumbCache.images;
  }

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
  connect(onAttempt?: (n: number) => void): Promise<void> {
    return this.signaling.connect(onAttempt);
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
    this.transport.close();
    this.views.clear();
    this.watching.clear();
  }

  leave() {
    if (this.status !== "closed") log.info("room", "Left the room");
    this.status = "closed";
    this.teardown();
    this.signaling.close();
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
        logNetwork("when joining the room");
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
        this.thumbCache.forget(msg.id);
        this.transport.removePeer(msg.id);
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
        this.thumbCache.forget(msg.id); // trocou de tela ou parou: a prévia antiga não vale mais
        if (!msg.sharing && msg.id !== this.selfId) this.dropView(msg.id);
        break;
      }
      case "signal":
        this.onSignal(msg.from, msg.data);
        break;
    }
    this.emit();
  }

  private onSignal(from: string, data: SignalData) {
    try {
      if (data.kind === "thumb-request" || data.kind === "thumb") {
        this.thumbCache.handleSignal(from, data, this.localStream);
      } else {
        this.transport.handleSignal(from, data);
      }
    } catch (e) {
      log.error("room", `Something broke while handling "${data.kind}" from ${this.nameOf(from)}: ${describeError(e)}`);
    }
  }

  // ---------- transmitir ----------

  private publication(capture: NativeCapture): Publication {
    return {
      stream: capture.stream,
      quality: this.quality,
      hwEncode: this.hwEncode,
      sourceHeight: () => this.capture?.frameHeight ?? 0,
    };
  }

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
    this.transport.publish(this.publication(capture));

    const label = `${qualityLabel(quality)} · ${sourceTitle}`.slice(0, 48);
    log.info(
      "sharing",
      `Started sharing "${sourceTitle}" at ${qualityLabel(quality)}, ` +
        `preferring ${hwEncode ? "H.264 (GPU)" : "VP8 (CPU)"}. Tracks: ${describeTracks(capture.stream)}`,
    );
    const me = this.peers.get(this.selfId)!;
    me.sharing = true;
    me.label = label;
    this.signaling.send({ type: "sharing", sharing: true, label });
    this.emit();
  }

  /**
   * Solta a captura atual sem derrubar as conexões, para trocar de tela: o
   * Rust só mantém uma captura por vez, e a próxima precisa da vaga.
   */
  releaseCapture() {
    if (!this.capture) return;
    this.capture.onEnded = null;
    this.capture.stop();
    this.capture = null;
  }

  /** Troca a tela transmitida sem desconectar quem está assistindo. */
  switchShare(capture: NativeCapture, quality: Quality, sourceTitle: string, hwEncode: boolean) {
    if (!this.localStream || this.status !== "connected") {
      this.startShare(capture, quality, sourceTitle, hwEncode);
      return;
    }
    this.releaseCapture();
    this.capture = capture;
    this.quality = quality;
    this.hwEncode = hwEncode;
    this.localStream = capture.stream;
    this.thumbCache.resetOwn();
    capture.onEnded = () => this.stopShare();
    const sameKinds = this.transport.replace(this.publication(capture));

    const label = `${qualityLabel(quality)} · ${sourceTitle}`.slice(0, 48);
    log.info(
      "sharing",
      `Switched to "${sourceTitle}" at ${qualityLabel(quality)}` +
        (sameKinds ? "" : " (audio changed, reconnecting viewers)") +
        `. Tracks: ${describeTracks(capture.stream)}`,
    );
    const me = this.peers.get(this.selfId)!;
    me.label = label;
    this.signaling.send({ type: "sharing", sharing: true, label });
    this.emit();
  }

  stopShare() {
    if (!this.localStream) return;
    log.info("sharing", "Stopped sharing");
    this.thumbCache.resetOwn();
    this.capture?.stop();
    this.capture = null;
    this.localStream = null;
    this.transport.unpublish();
    this.watching.delete(this.selfId);
    const me = this.peers.get(this.selfId);
    if (me) {
      me.sharing = false;
      me.label = null;
    }
    this.signaling.send({ type: "sharing", sharing: false });
    this.emit();
  }

  // ---------- assistir ----------

  watch(id: string) {
    this.watching.add(id);
    if (id !== this.selfId) {
      log.info("watching", `Asked to watch ${this.nameOf(id)}`);
      this.views.set(id, { stream: null, state: "connecting" });
      this.transport.watch(id);
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
    log.warn("watching", `Still connecting to ${this.nameOf(id)} after 20s: ${this.transport.describeStuck(id)}`);
  }

  unwatch(id: string) {
    this.dropView(id, id !== this.selfId && this.watching.has(id));
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

  /** Pede a prévia da tela de alguém, se a guardada tiver mais de 5 minutos. */
  requestThumb(id: string) {
    this.thumbCache.request(id);
  }

  isThumbLoading(id: string) {
    return this.thumbCache.isLoading(id);
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

  /** Avisa quem transmite o tamanho em que estou vendo (o transporte só manda se mudou). */
  private sendView(id: string) {
    if (id === this.selfId || !this.watching.has(id)) return;
    const pip = this.pip?.id === id ? this.pip.height : 0;
    const height = viewerHeight(this.viewHeight.get(id), pip, document.hidden);
    if (height !== null) this.transport.requestView(id, height);
  }

  /** Para de mostrar alguém; `tell` avisa a pessoa (não precisa se ela saiu ou parou). */
  private dropView(id: string, tell = false) {
    this.watching.delete(id);
    this.views.delete(id);
    this.transport.stopWatching(id, tell);
  }

  private setView(id: string, patch: Partial<RemoteView>) {
    const v = this.views.get(id);
    if (!v) return;
    this.views.set(id, { ...v, ...patch });
    this.emit();
  }
}
