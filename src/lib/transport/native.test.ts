// O transporte com envio nativo: confere que as mensagens da sala viram
// comandos para o Rust e que os eventos do Rust viram mensagens da sala, e
// que quem assiste (a malha normal, com WebRTC falso) conversa com ele.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TransportSignal } from "./types";
import { FakePC, installFakeRtc, settle, stream } from "../testing/fakeRtc";

const calls: { cmd: string; args: Record<string, unknown> }[] = [];
let emit: ((payload: unknown) => void) | null = null;

vi.mock("@tauri-apps/api/core", () => ({
  invoke: async (cmd: string, args: Record<string, unknown>) => {
    calls.push({ cmd, args });
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

const { NativeTransport } = await import("./native");
const { MeshTransport } = await import("./mesh");

function setup() {
  const sent: { to: string; data: TransportSignal }[] = [];
  const t = new NativeTransport({
    signal: (to, data) => sent.push({ to, data }),
    nameOf: (id) => `"${id}"`,
    onRemote: () => {},
  });
  t.publish({ stream: stream("video"), quality: { height: 1080, fps: 60 }, hwEncode: true, sourceHeight: () => 1080 });
  return { t, sent };
}

beforeEach(() => {
  installFakeRtc();
  calls.length = 0;
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("NativeTransport", () => {
  it("quem pede para assistir abre uma conexão no Rust e a oferta vai pela sala", async () => {
    const { t, sent } = setup();
    await settle();
    t.handleSignal("bia", { kind: "watch" });
    expect(calls).toContainEqual({ cmd: "native_viewer_open", args: { viewer: "bia", name: '"bia"' } });

    emit!({ viewer: "bia", kind: "offer", sdp: "v=0 offer" });
    emit!({ viewer: "bia", kind: "ice", candidate: "candidate:1 1 udp 1 1.2.3.4 5 typ srflx", mid: "0" });
    expect(sent).toEqual([
      { to: "bia", data: { kind: "offer", sdp: "v=0 offer" } },
      {
        to: "bia",
        data: {
          kind: "ice",
          role: "sharer",
          candidate: { candidate: "candidate:1 1 udp 1 1.2.3.4 5 typ srflx", sdpMid: "0", sdpMLineIndex: 0 },
        },
      },
    ]);
  });

  it("resposta e rotas de quem assiste vão para o Rust; ao sair, fecha", async () => {
    const { t } = setup();
    t.handleSignal("bia", { kind: "watch" });
    t.handleSignal("bia", { kind: "answer", sdp: "v=0 answer" });
    t.handleSignal("bia", { kind: "ice", role: "viewer", candidate: { candidate: "candidate:2 1 udp 1 5.6.7.8 9 typ host" } });
    t.handleSignal("bia", { kind: "view", height: 720 });
    t.handleSignal("bia", { kind: "unwatch" });
    t.unpublish();
    expect(calls.map((c) => c.cmd)).toEqual([
      "native_viewer_open",
      "native_viewer_answer",
      "native_viewer_candidate",
      "native_viewer_close",
      "native_send_stop",
    ]);
    expect(calls[1].args).toEqual({ viewer: "bia", sdp: "v=0 answer" });
  });

  it("ignora resposta de quem não tem conexão nativa e pedidos depois de parar", () => {
    const { t } = setup();
    t.handleSignal("ana", { kind: "answer", sdp: "x" });
    t.unpublish();
    calls.length = 0;
    t.handleSignal("bia", { kind: "watch" });
    expect(calls).toEqual([]);
  });

  it("assistir os outros continua pela malha", async () => {
    const { t, sent } = setup();
    t.watch("caio");
    t.handleSignal("caio", { kind: "offer", sdp: "v=0\r\n" });
    await settle();
    expect(sent.map((m) => m.data.kind)).toEqual(["watch", "answer"]);
    expect(FakePC.all).toHaveLength(1);
  });

  it("conversa com um espectador de verdade (a malha) do outro lado", async () => {
    // quem transmite: envio nativo; quem assiste: malha normal
    const queue: { from: string; to: string; data: TransportSignal }[] = [];
    const sharer = new NativeTransport({
      signal: (to, data) => queue.push({ from: "ana", to, data }),
      nameOf: (id) => id,
      onRemote: () => {},
    });
    sharer.publish({ stream: stream("video"), quality: { height: 720, fps: 30 }, hwEncode: true, sourceHeight: () => 720 });
    const viewer = new MeshTransport({
      signal: (to, data) => queue.push({ from: "bia", to, data }),
      nameOf: (id) => id,
      onRemote: () => {},
    });
    const nodes: Record<string, { handleSignal: (f: string, d: TransportSignal) => void }> = { ana: sharer, bia: viewer };
    const pump = async () => {
      for (let i = 0; i < 10; i++) {
        await settle(2);
        for (const m of queue.splice(0)) nodes[m.to].handleSignal(m.from, m.data);
      }
    };
    await settle();

    viewer.watch("ana");
    await pump();
    // o Rust responderia com a oferta
    emit!({ viewer: "bia", kind: "offer", sdp: "v=0\r\na=rtpmap:108 H264/90000\r\n" });
    await pump();

    const viewerPc = FakePC.all[0];
    expect(viewerPc.remoteDescription?.type).toBe("offer");
    expect(calls.map((c) => c.cmd)).toEqual(["native_viewer_open", "native_viewer_answer"]);
  });
});
