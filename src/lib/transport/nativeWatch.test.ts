// A recepção nativa vista pelo app: confere que as mensagens da sala viram
// comandos para o Rust, que os eventos do Rust viram mensagens da sala e que
// a mídia do WebSocket local chega ao decodificador. O Rust, o WebSocket e o
// decodificador são simulados.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RemoteView, TransportSignal } from "./types";
import { settle, stream } from "../testing/fakeRtc";

const calls: { cmd: string; args: Record<string, unknown> }[] = [];
let emit: ((payload: unknown) => void) | null = null;
let openFails = false;

vi.mock("@tauri-apps/api/core", () => ({
  invoke: async (cmd: string, args: Record<string, unknown>) => {
    calls.push({ cmd, args });
    if (cmd === "native_watch_open") {
      if (openFails) throw new Error("offer refused");
      return "ws://127.0.0.1:5000/watch/token";
    }
  },
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: async (_name: string, cb: (e: { payload: unknown }) => void) => {
    emit = (payload) => cb({ payload });
    return () => (emit = null);
  },
}));
vi.mock("../log", () => ({
  log: { info: () => {}, warn: () => {}, error: () => {} },
  describeError: (e: unknown) => String(e),
}));

type FakeDecoded = {
  stream: MediaStream;
  pushed: ArrayBuffer[];
  closed: boolean;
  onUnsupported: (() => void) | null;
  push: (buf: ArrayBuffer) => void;
  close: () => void;
  stats: () => object;
};
let decoded: FakeDecoded[] = [];
vi.mock("../watch/decode", () => ({
  createDecodedStream: async () => {
    const d: FakeDecoded = {
      stream: stream("video", "audio"),
      pushed: [],
      closed: false,
      onUnsupported: null,
      push: (buf) => d.pushed.push(buf),
      close: () => (d.closed = true),
      stats: () => ({ decoded: 0, dropped: 0, width: 0, height: 0 }),
    };
    decoded.push(d);
    return d;
  },
}));

class FakeSocket {
  static all: FakeSocket[] = [];
  binaryType = "";
  onmessage: ((ev: { data: ArrayBuffer }) => void) | null = null;
  closed = false;
  constructor(public url: string) {
    FakeSocket.all.push(this);
  }
  close() {
    this.closed = true;
  }
}

const { NativeWatcher } = await import("./nativeWatch");

function setup() {
  const sent: { to: string; data: TransportSignal }[] = [];
  const views: { id: string; patch: Partial<RemoteView> }[] = [];
  const w = new NativeWatcher({
    signal: (to, data) => sent.push({ to, data }),
    nameOf: (id) => `"${id}"`,
    onRemote: (id, patch) => views.push({ id, patch }),
    iceServers: () => [],
  });
  return { w, sent, views };
}

beforeEach(() => {
  vi.stubGlobal("WebSocket", FakeSocket);
  calls.length = 0;
  decoded = [];
  FakeSocket.all = [];
  openFails = false;
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("NativeWatcher", () => {
  it("pede para assistir e, com a oferta, abre a recepção no Rust e entrega a tela", async () => {
    const { w, sent, views } = setup();
    await settle();
    w.watch("ana");
    expect(sent).toEqual([{ to: "ana", data: { kind: "watch" } }]);

    w.handleSignal("ana", { kind: "offer", sdp: "v=0 offer" });
    await settle();
    expect(calls).toContainEqual({ cmd: "native_watch_open", args: { sharer: "ana", name: '"ana"', sdp: "v=0 offer" } });
    expect(FakeSocket.all.map((s) => [s.url, s.binaryType])).toEqual([["ws://127.0.0.1:5000/watch/token", "arraybuffer"]]);
    expect(views).toEqual([{ id: "ana", patch: { stream: decoded[0].stream } }]);

    // a mídia do WebSocket local vai para o decodificador
    const packet = new ArrayBuffer(20);
    FakeSocket.all[0].onmessage!({ data: packet });
    expect(decoded[0].pushed).toEqual([packet]);
  });

  it("a resposta, as rotas e o estado que o Rust avisa viram mensagens da sala", async () => {
    const { w, sent, views } = setup();
    await settle();
    w.watch("ana");
    w.handleSignal("ana", { kind: "offer", sdp: "v=0 offer" });
    await settle();
    sent.length = 0;
    views.length = 0;

    emit!({ sharer: "ana", kind: "answer", sdp: "v=0 answer" });
    emit!({ sharer: "ana", kind: "ice", candidate: "candidate:1 1 udp 1 1.2.3.4 5 typ srflx", mid: "0" });
    emit!({ sharer: "ana", kind: "state", state: "connected" });
    expect(sent).toEqual([
      { to: "ana", data: { kind: "answer", sdp: "v=0 answer" } },
      {
        to: "ana",
        data: {
          kind: "ice",
          role: "viewer",
          candidate: { candidate: "candidate:1 1 udp 1 1.2.3.4 5 typ srflx", sdpMid: "0", sdpMLineIndex: 0 },
        },
      },
    ]);
    expect(views).toEqual([{ id: "ana", patch: { state: "connected" } }]);

    emit!({ sharer: "ana", kind: "state", state: "failed" });
    expect(views.at(-1)).toEqual({ id: "ana", patch: { state: "failed" } });
  });

  it("as rotas de quem transmite vão para o Rust, depois de a recepção abrir", async () => {
    const { w } = setup();
    await settle();
    w.watch("ana");
    w.handleSignal("ana", { kind: "offer", sdp: "v=0 offer" });
    // chega antes de o Rust responder ao `native_watch_open`
    w.handleSignal("ana", { kind: "ice", role: "sharer", candidate: { candidate: "candidate:9 1 udp 1 5.6.7.8 9 typ host" } });
    await settle();
    const order = calls.map((c) => c.cmd);
    expect(order).toEqual(["native_watch_open", "native_watch_candidate"]);
    expect(calls[1].args).toEqual({ sharer: "ana", candidate: "candidate:9 1 udp 1 5.6.7.8 9 typ host" });
  });

  it("uma oferta de quem eu não pedi para assistir é ignorada", async () => {
    const { w, views } = setup();
    await settle();
    w.handleSignal("bia", { kind: "offer", sdp: "v=0 offer" });
    await settle();
    expect(calls).toEqual([]);
    expect(views).toEqual([]);
  });

  it("parar de assistir avisa a pessoa e fecha tudo", async () => {
    const { w, sent } = setup();
    await settle();
    w.watch("ana");
    w.handleSignal("ana", { kind: "offer", sdp: "v=0 offer" });
    await settle();
    w.stopWatching("ana", true);
    await settle();
    expect(sent.at(-1)).toEqual({ to: "ana", data: { kind: "unwatch" } });
    expect(calls.at(-1)).toEqual({ cmd: "native_watch_close", args: { sharer: "ana" } });
    expect(decoded[0].closed).toBe(true);
    expect(FakeSocket.all[0].closed).toBe(true);
  });

  it("uma oferta nova substitui a recepção anterior", async () => {
    const { w } = setup();
    await settle();
    w.watch("ana");
    w.handleSignal("ana", { kind: "offer", sdp: "v=0 first" });
    await settle();
    w.handleSignal("ana", { kind: "offer", sdp: "v=0 second", restart: true });
    await settle();
    expect(decoded[0].closed).toBe(true);
    expect(decoded[1].closed).toBe(false);
    expect(calls.filter((c) => c.cmd === "native_watch_open").map((c) => c.args.sdp)).toEqual(["v=0 first", "v=0 second"]);
  });

  it("sem os codecs do sistema, a tela falha dizendo o motivo", async () => {
    const { w, views } = setup();
    await settle();
    w.watch("ana");
    w.handleSignal("ana", { kind: "offer", sdp: "v=0 offer" });
    await settle();
    decoded[0].onUnsupported!();
    expect(views.at(-1)).toEqual({ id: "ana", patch: { state: "failed", problem: "codecs" } });
  });

  it("se o Rust recusar a oferta, a tela falha em vez de ficar conectando", async () => {
    openFails = true;
    const { w, views } = setup();
    await settle();
    w.watch("ana");
    w.handleSignal("ana", { kind: "offer", sdp: "v=0 offer" });
    await settle();
    expect(views.at(-1)).toEqual({ id: "ana", patch: { state: "failed" } });
    expect(decoded[0].closed).toBe(true);
  });

  it("pede a qualidade do tamanho em que estou vendo, só quando muda", async () => {
    const { w, sent } = setup();
    await settle();
    w.watch("ana");
    w.requestView("ana", 480);
    w.requestView("ana", 480);
    w.requestView("ana", 720);
    expect(sent.filter((s) => s.data.kind === "view").map((s) => s.data)).toEqual([
      { kind: "view", height: 480 },
      { kind: "view", height: 720 },
    ]);
  });
});

describe("NativeTransport sem WebRTC no WebView", () => {
  it("assiste pela recepção nativa em vez da malha", async () => {
    // sem `installFakeRtc`: não existe RTCPeerConnection, como no WebKitGTK
    const { NativeTransport } = await import("./native");
    const sent: { to: string; data: TransportSignal }[] = [];
    const t = new NativeTransport({
      signal: (to, data) => sent.push({ to, data }),
      nameOf: (id) => `"${id}"`,
      onRemote: () => {},
      iceServers: () => [],
    });
    await settle();
    t.watch("ana");
    t.handleSignal("ana", { kind: "offer", sdp: "v=0 offer" });
    await settle();
    expect(calls.map((c) => c.cmd)).toContain("native_watch_open");
    t.close();
  });
});
