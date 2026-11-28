// Assistir com a recepção nativa: onde o WebView não tem WebRTC (Linux,
// WebKitGTK), quem recebe a tela dos outros é o Rust (`src-tauri/src/receive/`).
// Este lado só leva as mensagens entre o Rust e a sala e entrega a mídia, que
// chega ainda codificada por um WebSocket local, ao decodificador
// (`watch/decode.ts`). Para quem transmite não muda nada: as mensagens
// (offer/answer/ice) são as mesmas do WebRTC do WebView.
//
// Sem repasse (TURN) deste lado: o str0m não tem cliente TURN. Funciona
// quando quem transmite tem a rota de repasse (o WebView dele) ou há caminho
// direto.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { describeError, log } from "../log";
import { createDecodedStream, type DecodedStream } from "../watch/decode";
import { armSound } from "../watch/sound";
import type { TransportHost, TransportSignal, Watcher } from "./types";

/** O que o Rust manda (evento `native-watch`). */
export type NativeWatchEvent = { sharer: string } & (
  | { kind: "answer"; sdp: string }
  | { kind: "ice"; candidate: string; mid: string }
  | { kind: "state"; state: "connected" | "disconnected" | "failed" | "closed" }
);

/** A recepção da tela de uma pessoa. */
type Reception = {
  /** Termina quando o Rust aceitou a oferta (`true`) ou recusou. */
  opened: Promise<boolean>;
  decoded: DecodedStream | null;
  socket: WebSocket | null;
  closed: boolean;
  state: string;
};

export class NativeWatcher implements Watcher {
  private watching = new Set<string>();
  private receptions = new Map<string, Reception>();
  /** O que já avisei a cada pessoa (altura), para não repetir. */
  private sentView = new Map<string, number>();
  private unlisten: Promise<UnlistenFn>;

  constructor(private host: TransportHost) {
    this.unlisten = listen<NativeWatchEvent>("native-watch", (e) => this.onNative(e.payload));
    // o som só sai se for acordado num gesto; o clique em Assistir é um deles
    armSound();
    log.info("watching", "Using native receiving: this WebView has no WebRTC, so the app itself receives the screens");
  }

  private call(cmd: string, args: Record<string, unknown>, what: string): Promise<unknown> {
    return invoke(cmd, args).catch((e) => log.error("watching", `${what} failed: ${describeError(e)}`));
  }

  // ---------- assistir ----------

  watch(peerId: string) {
    this.watching.add(peerId);
    this.host.signal(peerId, { kind: "watch" });
    this.sentView.delete(peerId);
  }

  stopWatching(peerId: string, tell: boolean) {
    if (tell && this.watching.has(peerId)) this.host.signal(peerId, { kind: "unwatch" });
    this.watching.delete(peerId);
    this.sentView.delete(peerId);
    this.closeReception(peerId, tell ? "stopped watching" : "they left");
  }

  requestView(peerId: string, height: number) {
    if (this.sentView.get(peerId) === height) return;
    this.sentView.set(peerId, height);
    this.host.signal(peerId, { kind: "view", height });
  }

  describeStuck(peerId: string): string {
    const reception = this.receptions.get(peerId);
    if (!reception) return "their PC never sent an offer (the problem is on their side, check their log)";
    const stats = reception.decoded?.stats();
    return (
      `offer received, the native connection is "${reception.state}"` +
      (stats ? `, ${stats.decoded} frames decoded and ${stats.dropped} dropped so far` : ", the decoder is not ready yet")
    );
  }

  private closeReception(peerId: string, why: string) {
    const reception = this.receptions.get(peerId);
    if (!reception) return;
    this.receptions.delete(peerId);
    reception.closed = true;
    reception.socket?.close();
    reception.decoded?.close();
    log.info("watching", `Closed the native reception from ${this.host.nameOf(peerId)} (${why})`);
    this.call("native_watch_close", { sharer: peerId }, `Closing the connection from ${this.host.nameOf(peerId)}`);
  }

  private acceptOffer(from: string, sdp: string) {
    if (!this.watching.has(from)) {
      log.warn("watching", `Got an offer from ${this.host.nameOf(from)}, but I did not ask to watch them; ignoring it`);
      return;
    }
    // uma oferta nova (ex.: rotas novas de quem transmite) recomeça a recepção
    this.closeReception(from, "a new offer replaced it");
    log.info("watching", `Got the offer from ${this.host.nameOf(from)} (native receiving)`);
    const reception: Reception = { opened: Promise.resolve(false), decoded: null, socket: null, closed: false, state: "connecting" };
    this.receptions.set(from, reception);
    reception.opened = this.open(from, sdp, reception);
  }

  private async open(from: string, sdp: string, reception: Reception): Promise<boolean> {
    let decoded: DecodedStream | null = null;
    try {
      decoded = await createDecodedStream();
      const url = await invoke<string>("native_watch_open", { sharer: from, name: this.host.nameOf(from), sdp });
      if (reception.closed) {
        // parei de assistir enquanto abria
        decoded.close();
        return false;
      }
      reception.decoded = decoded;
      decoded.onUnsupported = () => {
        if (this.receptions.get(from) === reception) this.host.onRemote(from, { state: "failed", problem: "codecs" });
      };
      const socket = new WebSocket(url);
      socket.binaryType = "arraybuffer";
      socket.onmessage = (ev) => decoded!.push(ev.data as ArrayBuffer);
      reception.socket = socket;
      this.host.onRemote(from, { stream: decoded.stream, setVolume: decoded.setVolume });
      return true;
    } catch (e) {
      log.error("watching", `Could not start receiving from ${this.host.nameOf(from)}: ${describeError(e)}`);
      decoded?.close();
      if (this.receptions.get(from) === reception) {
        reception.state = "failed";
        this.host.onRemote(from, { state: "failed" });
      }
      return false;
    }
  }

  private onNative(ev: NativeWatchEvent) {
    const { sharer } = ev;
    switch (ev.kind) {
      case "answer":
        this.host.signal(sharer, { kind: "answer", sdp: ev.sdp });
        break;
      case "ice":
        this.host.signal(sharer, {
          kind: "ice",
          role: "viewer",
          candidate: { candidate: ev.candidate, sdpMid: ev.mid, sdpMLineIndex: 0 },
        });
        break;
      case "state": {
        const reception = this.receptions.get(sharer);
        if (!reception) break;
        reception.state = ev.state;
        if (ev.state === "connected" || ev.state === "failed") this.host.onRemote(sharer, { state: ev.state });
        break;
      }
    }
  }

  // ---------- geral ----------

  handleSignal(from: string, data: TransportSignal) {
    switch (data.kind) {
      case "offer":
        this.acceptOffer(from, data.sdp);
        break;
      case "ice": {
        if (data.role !== "sharer" || !data.candidate.candidate) break;
        const reception = this.receptions.get(from);
        if (!reception) {
          log.warn("watching", `Got a network route from ${this.host.nameOf(from)}, but no connection with them is open; ignoring it`);
          break;
        }
        const candidate = data.candidate.candidate;
        reception.opened.then((ok) => {
          if (ok && !reception.closed) {
            this.call("native_watch_candidate", { sharer: from, candidate }, `Adding a network route from ${this.host.nameOf(from)}`);
          }
        });
        break;
      }
    }
  }

  /** Sem repasse deste lado: nada a atualizar. */
  refreshIceServers() {}

  removePeer(peerId: string) {
    this.stopWatching(peerId, false);
  }

  close() {
    [...this.receptions.keys()].forEach((id) => this.closeReception(id, "left the room"));
    this.watching.clear();
    this.unlisten.then((stop) => stop()).catch(() => {});
  }
}
