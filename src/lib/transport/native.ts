// Transporte com envio nativo (ROADMAP, seção 2, parte 3b): quem transmite
// codifica e manda tudo no Rust (`src-tauri/src/send/`); este lado só leva as
// mensagens entre o Rust e a sala. Assistir os outros continua igual (a
// malha, `mesh.ts`), e quem assiste não percebe diferença: as mensagens
// (offer/answer/ice) são as mesmas do WebRTC do WebView.
//
// Vídeo (H.264) e som (Opus) saem do Rust no mesmo stream. Ligado só pela preferência
// escondida `nativeSend`.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { describeError, log } from "../log";
import { MeshTransport } from "./mesh";
import type { MediaTransport, Publication, TransportHost, TransportSignal } from "./types";

/** O que o Rust manda (evento `native-send`). */
export type NativeEvent = { viewer: string } & (
  | { kind: "offer"; sdp: string }
  | { kind: "ice"; candidate: string; mid: string }
  | { kind: "state"; state: "connected" | "disconnected" | "failed" | "closed" }
);

export class NativeTransport implements MediaTransport {
  private mesh: MeshTransport;
  private sharing = false;
  /** Quem está com uma conexão nativa aberta (e quando ela terminou de abrir). */
  private viewers = new Map<string, Promise<unknown>>();
  private unlisten: Promise<UnlistenFn>;

  constructor(private host: TransportHost) {
    this.mesh = new MeshTransport(host);
    this.unlisten = listen<NativeEvent>("native-send", (e) => this.onNative(e.payload));
    log.info("sharing", "Using native sending (experimental): my screen is encoded and sent by the app itself");
  }

  private call(cmd: string, args: Record<string, unknown>, what: string): Promise<unknown> {
    return invoke(cmd, args).catch((e) => log.error("sharing", `${what} failed: ${describeError(e)}`));
  }

  // ---------- transmitir (no Rust) ----------

  publish(_pub: Publication) {
    this.sharing = true;
  }

  replace(_pub: Publication): boolean {
    // a captura nova alimenta o mesmo envio no Rust: ninguém desconecta
    return true;
  }

  unpublish() {
    this.sharing = false;
    this.viewers.clear();
    this.call("native_send_stop", {}, "Stopping native sending");
  }

  private openViewer(id: string) {
    if (!this.sharing) {
      log.warn("sharing", `${this.host.nameOf(id)} wants to watch, but I'm not sharing anymore`);
      return;
    }
    log.info("sharing", `${this.host.nameOf(id)} wants to watch (native sending)`);
    const opened = this.call(
      "native_viewer_open",
      { viewer: id, name: this.host.nameOf(id) },
      `Opening the connection to ${this.host.nameOf(id)}`,
    );
    this.viewers.set(id, opened);
  }

  /** O tamanho em que a pessoa está vendo: o Rust escolhe a camada (só depois de abrir a conexão). */
  private view(id: string, height: number) {
    const opened = this.viewers.get(id);
    if (!opened) return;
    opened.then(() =>
      this.call("native_viewer_view", { viewer: id, height }, `Telling the size ${this.host.nameOf(id)} watches in`),
    );
  }

  private closeViewer(id: string) {
    if (!this.viewers.delete(id)) return;
    this.call("native_viewer_close", { viewer: id }, `Closing the connection to ${this.host.nameOf(id)}`);
  }

  private onNative(ev: NativeEvent) {
    const { viewer } = ev;
    switch (ev.kind) {
      case "offer":
        this.host.signal(viewer, { kind: "offer", sdp: ev.sdp });
        break;
      case "ice":
        this.host.signal(viewer, {
          kind: "ice",
          role: "sharer",
          candidate: { candidate: ev.candidate, sdpMid: ev.mid, sdpMLineIndex: 0 },
        });
        break;
      case "state":
        if (ev.state === "closed" || ev.state === "failed") this.viewers.delete(viewer);
        break;
    }
  }

  // ---------- assistir (igual à malha) ----------

  watch(peerId: string) {
    this.mesh.watch(peerId);
  }

  stopWatching(peerId: string, tell: boolean) {
    this.mesh.stopWatching(peerId, tell);
  }

  requestView(peerId: string, height: number) {
    this.mesh.requestView(peerId, height);
  }

  describeStuck(peerId: string): string {
    return this.mesh.describeStuck(peerId);
  }

  // ---------- geral ----------

  handleSignal(from: string, data: TransportSignal) {
    switch (data.kind) {
      case "watch":
        this.openViewer(from);
        return;
      case "unwatch":
        log.info("sharing", `${this.host.nameOf(from)} stopped watching`);
        this.closeViewer(from);
        return;
      case "view":
        // qualidade por espectador: camadas no Rust (ROADMAP, parte 3d)
        this.view(from, Math.max(0, Number(data.height) || 0));
        return;
      case "answer":
        if (!this.viewers.has(from)) {
          log.warn("sharing", `Got an answer from ${this.host.nameOf(from)}, but no native connection was waiting for it`);
          return;
        }
        log.info("sharing", `Got the answer from ${this.host.nameOf(from)} (native sending)`);
        this.call("native_viewer_answer", { viewer: from, sdp: data.sdp }, `Applying the answer from ${this.host.nameOf(from)}`);
        return;
      case "ice":
        if (data.role === "viewer") {
          if (this.viewers.has(from) && data.candidate.candidate) {
            this.call(
              "native_viewer_candidate",
              { viewer: from, candidate: data.candidate.candidate },
              `Adding a network route from ${this.host.nameOf(from)}`,
            );
          }
          return;
        }
        this.mesh.handleSignal(from, data);
        return;
      case "offer":
        this.mesh.handleSignal(from, data);
        return;
    }
  }

  removePeer(peerId: string) {
    this.closeViewer(peerId);
    this.mesh.removePeer(peerId);
  }

  close() {
    if (this.sharing) this.unpublish();
    this.mesh.close();
    this.unlisten.then((stop) => stop()).catch(() => {});
  }
}
