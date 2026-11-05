// Transporte em malha: cada "assistir" é uma RTCPeerConnection própria (quem
// transmite -> quem assiste), então quem transmite só gasta upload com quem
// está vendo, e codifica uma vez por espectador.

import { describeError, log } from "../log";
import { encodingFor, qualityLabel, rankVideoCodec, videoBitrate } from "../quality";
import {
  describeTracks,
  instrument,
  logConnectionReport,
  noteClosed,
  noteLocalRoute,
  noteRemoteRoute,
  type Instrumented,
} from "./diagnostics";
import { describeSdp, summarizeRoutes, type Route } from "./routes";
import { AUDIO_BITRATE, tuneOpus } from "./sdp";
import type { MediaTransport, Publication, TransportHost, TransportSignal } from "./types";

const RTC_CONFIG: RTCConfiguration = {
  iceServers: [
    { urls: ["stun:stun.cloudflare.com:3478", "stun:stun.l.google.com:19302"] },
  ],
};

/** Executa as operações de uma conexão em ordem (evita corridas de SDP/ICE). */
class Conn implements Instrumented {
  private chain: Promise<void> = Promise.resolve();
  /** Última qualidade aplicada para este espectador (para logar só mudanças). */
  applied = "";
  localRoutes: Route[] = [];
  remoteRoutes: Route[] = [];
  startedAt = Date.now();
  constructor(
    public pc: RTCPeerConnection,
    public area: "sharing" | "watching",
    public who: string,
  ) {}
  run(step: string, fn: () => Promise<void>) {
    this.chain = this.chain
      .then(fn)
      .catch((e) => log.error(this.area, `${step} failed (${this.who}): ${describeError(e)}`));
  }
}

/** Põe os codecs de vídeo na ordem de `rankVideoCodec`. */
function preferCodecs(transceiver: RTCRtpTransceiver, hw: boolean) {
  const codecs = RTCRtpSender.getCapabilities("video")?.codecs;
  if (!codecs || !transceiver.setCodecPreferences) return;
  const sorted = [...codecs].sort((a, b) => rankVideoCodec(a, hw) - rankVideoCodec(b, hw));
  try {
    transceiver.setCodecPreferences(sorted);
  } catch (e) {
    log.warn("sharing", `Could not set the video codec order: ${describeError(e)}`);
  }
}

const trackKinds = (s: MediaStream) => s.getTracks().map((t) => t.kind).sort().join();

export class MeshTransport implements MediaTransport {
  private pub: Publication | null = null;
  private out = new Map<string, Conn>(); // eu transmito -> viewer
  private inc = new Map<string, Conn>(); // sharer -> eu assisto
  /** Quem eu pedi para assistir (só aceito oferta dessas pessoas). */
  private watching = new Set<string>();
  /** Quem assiste: a altura que já avisei a cada pessoa. */
  private sentView = new Map<string, number>();
  /** Quem transmite: altura pedida por cada espectador. */
  private viewerWants = new Map<string, number>();

  constructor(private host: TransportHost) {}

  private signal(to: string, data: TransportSignal) {
    this.host.signal(to, data);
  }

  private nameOf(id: string) {
    return this.host.nameOf(id);
  }

  // ---------- transmitir ----------

  publish(pub: Publication) {
    this.pub = pub;
  }

  replace(pub: Publication): boolean {
    const old = this.pub?.stream;
    this.pub = pub;
    // mesmas faixas (vídeo, áudio): só troca o conteúdo; senão, refaz a conexão
    const sameKinds = !!old && trackKinds(old) === trackKinds(pub.stream);
    // `startSending` recria a entrada no mapa: percorre uma cópia das chaves,
    // senão a entrada recriada é visitada de novo e o laço não termina
    for (const viewerId of [...this.out.keys()]) {
      const conn = this.out.get(viewerId)!;
      if (!sameKinds) {
        this.startSending(viewerId);
        continue;
      }
      conn.run("Switching the screen", async () => {
        for (const sender of conn.pc.getSenders()) {
          const track = pub.stream.getTracks().find((t) => t.kind === sender.track?.kind);
          if (track) await sender.replaceTrack(track);
        }
      });
      this.applyView(viewerId); // a qualidade nova pode ter outro fps e bitrate
    }
    return sameKinds;
  }

  unpublish() {
    this.pub = null;
    this.out.forEach((c) => {
      noteClosed(c, "stopped sharing");
      c.pc.close();
    });
    this.out.clear();
  }

  private closeOut(viewerId: string, why: string) {
    const conn = this.out.get(viewerId);
    if (!conn) return;
    noteClosed(conn, why);
    conn.pc.close();
    this.out.delete(viewerId);
  }

  private startSending(viewerId: string) {
    const who = `to ${this.nameOf(viewerId)}`;
    const pub = this.pub;
    if (!pub) {
      log.warn("sharing", `${this.nameOf(viewerId)} wants to watch, but I'm not sharing anymore`);
      return;
    }
    const { stream, quality } = pub;
    log.info("sharing", `${this.nameOf(viewerId)} wants to watch. Tracks: ${describeTracks(stream)}`);
    this.closeOut(viewerId, "starting over for a new request");

    const pc = new RTCPeerConnection(RTC_CONFIG);
    const conn = new Conn(pc, "sharing", who);
    this.out.set(viewerId, conn);
    instrument(conn);

    pc.onicecandidate = (e) => {
      if (e.candidate) {
        noteLocalRoute(conn, e.candidate);
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
              ? { maxBitrate: videoBitrate(quality), maxFramerate: quality.fps }
              : { maxBitrate: AUDIO_BITRATE },
          ],
        });
        if (track.kind === "video") preferCodecs(transceiver, pub.hwEncode);
      }
    } catch (e) {
      log.error("sharing", `Could not add the video/audio to the connection ${who}: ${describeError(e)}`);
      return;
    }
    conn.run("Creating the offer", async () => {
      const offer = await pc.createOffer();
      await pc.setLocalDescription({ type: "offer", sdp: tuneOpus(offer.sdp!) });
      this.signal(viewerId, { kind: "offer", sdp: pc.localDescription!.sdp });
      log.info("sharing", `Sent the offer ${who}: ${describeSdp(pc.localDescription!.sdp)}`);
    });
    if (this.viewerWants.has(viewerId)) this.applyView(viewerId);
  }

  /** Ajusta o vídeo que vai para um espectador ao tamanho em que ele está vendo. */
  private applyView(viewerId: string) {
    const conn = this.out.get(viewerId);
    if (!conn) return;
    conn.run("Adjusting the quality", async () => {
      const pub = this.pub;
      const sender = conn.pc.getSenders().find((s) => s.track?.kind === "video");
      const params = sender?.getParameters();
      const enc = params?.encodings?.[0];
      if (!pub || !sender || !params || !enc) return;
      const { quality } = pub;
      const wants = this.viewerWants.get(viewerId) ?? quality.height;
      const target = encodingFor(wants, quality, pub.sourceHeight() || quality.height);
      let summary: string;
      if (!target.active) {
        enc.active = false;
        summary = "paused my video for them (AbreTela is hidden on their PC), audio keeps going";
      } else {
        enc.active = true;
        enc.scaleResolutionDownBy = target.scaleDownBy;
        enc.maxFramerate = target.fps;
        enc.maxBitrate = target.maxBitrate;
        summary = `sending ${qualityLabel(target)}`;
      }
      if (conn.applied === summary) return;
      await sender.setParameters(params);
      conn.applied = summary;
      log.info("sharing", `${this.nameOf(viewerId)}: ${summary}`);
    });
  }

  // ---------- assistir ----------

  watch(peerId: string) {
    this.watching.add(peerId);
    this.signal(peerId, { kind: "watch" });
    this.sentView.delete(peerId);
  }

  stopWatching(peerId: string, tell: boolean) {
    if (tell && this.watching.has(peerId)) this.signal(peerId, { kind: "unwatch" });
    this.watching.delete(peerId);
    this.sentView.delete(peerId);
    const conn = this.inc.get(peerId);
    if (conn) {
      noteClosed(conn, tell ? "stopped watching" : "they left");
      conn.pc.close();
    }
    this.inc.delete(peerId);
  }

  requestView(peerId: string, height: number) {
    if (this.sentView.get(peerId) === height) return;
    this.sentView.set(peerId, height);
    this.signal(peerId, { kind: "view", height });
  }

  describeStuck(peerId: string): string {
    const conn = this.inc.get(peerId);
    if (!conn) return "their PC never sent an offer (the problem is on their side, check their log)";
    logConnectionReport(conn, "Still connecting", "warn").catch(() => {});
    const types = (routes: Route[]) => summarizeRoutes(routes.map((r) => r.type));
    return (
      `offer received, connection is "${conn.pc.connectionState}", ` +
      `routes here: ${types(conn.localRoutes)}, from them: ${types(conn.remoteRoutes)} (details below)`
    );
  }

  private acceptOffer(from: string, sdp: string) {
    if (!this.watching.has(from)) {
      log.warn("watching", `Got an offer from ${this.nameOf(from)}, but I did not ask to watch them; ignoring it`);
      return;
    }
    log.info("watching", `Got the offer from ${this.nameOf(from)}: ${describeSdp(sdp)}`);
    const old = this.inc.get(from);
    if (old) {
      noteClosed(old, "a new offer replaced it");
      old.pc.close();
    }
    const pc = new RTCPeerConnection(RTC_CONFIG);
    const conn = new Conn(pc, "watching", `from ${this.nameOf(from)}`);
    this.inc.set(from, conn);
    instrument(conn, (state) => {
      if (this.inc.get(from)?.pc !== pc) return;
      if (state === "connected") this.host.onRemote(from, { state: "connected" });
      if (state === "failed") this.host.onRemote(from, { state: "failed" });
    });

    pc.onicecandidate = (e) => {
      if (e.candidate) {
        noteLocalRoute(conn, e.candidate);
        this.signal(from, { kind: "ice", role: "viewer", candidate: e.candidate.toJSON() });
      }
    };
    pc.ontrack = (e) => {
      const stream = e.streams[0] ?? new MediaStream([e.track]);
      this.host.onRemote(from, { stream });
    };
    conn.run("Answering the offer", async () => {
      await pc.setRemoteDescription({ type: "offer", sdp });
      const answer = await pc.createAnswer();
      await pc.setLocalDescription({ type: "answer", sdp: tuneOpus(answer.sdp!) });
      this.signal(from, { kind: "answer", sdp: pc.localDescription!.sdp });
      log.info("watching", `Sent the answer to ${this.nameOf(from)}: ${describeSdp(pc.localDescription!.sdp)}`);
    });
  }

  // ---------- geral ----------

  handleSignal(from: string, data: TransportSignal) {
    switch (data.kind) {
      case "watch":
        this.startSending(from);
        break;
      case "unwatch":
        log.info("sharing", `${this.nameOf(from)} stopped watching`);
        this.viewerWants.delete(from);
        this.closeOut(from, "they stopped watching");
        break;
      case "view":
        this.viewerWants.set(from, Math.max(0, Number(data.height) || 0));
        this.applyView(from);
        break;
      case "offer":
        this.acceptOffer(from, data.sdp);
        break;
      case "answer": {
        const conn = this.out.get(from);
        if (!conn) {
          log.warn("sharing", `Got an answer from ${this.nameOf(from)}, but no connection was waiting for it`);
          break;
        }
        log.info("sharing", `Got the answer from ${this.nameOf(from)}: ${describeSdp(data.sdp)}`);
        conn.run("Applying the answer", () => conn.pc.setRemoteDescription({ type: "answer", sdp: data.sdp }));
        break;
      }
      case "ice": {
        const conn = data.role === "sharer" ? this.inc.get(from) : this.out.get(from);
        if (!conn) {
          log.warn(
            data.role === "sharer" ? "watching" : "sharing",
            `Got a network route from ${this.nameOf(from)}, but no connection with them is open; ignoring it`,
          );
          break;
        }
        noteRemoteRoute(conn, data.candidate);
        conn.run("Adding a network route", () => conn.pc.addIceCandidate(data.candidate));
        break;
      }
    }
  }

  removePeer(peerId: string) {
    this.viewerWants.delete(peerId);
    this.closeOut(peerId, "they left");
    this.stopWatching(peerId, false);
  }

  close() {
    this.pub = null;
    this.out.forEach((c) => {
      noteClosed(c, "left the room");
      c.pc.close();
    });
    this.inc.forEach((c) => {
      noteClosed(c, "left the room");
      c.pc.close();
    });
    this.out.clear();
    this.inc.clear();
    this.watching.clear();
    this.sentView.clear();
    this.viewerWants.clear();
  }
}
