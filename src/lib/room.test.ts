// O RoomClient inteiro (sinalização + transporte + prévias) com duas pessoas
// numa sala falsa que se comporta como o servidor em Rust (server.rs).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RoomClient } from "./room";
import { WrongPasswordError } from "./signaling";
import { FakePC, installFakeRtc, settle, stream } from "./testing/fakeRtc";
import type { NativeCapture } from "./capture/native";

vi.mock("./log", () => ({
  log: { info: () => {}, warn: () => {}, error: () => {} },
  describeError: (e: unknown) => String(e),
}));

const URL = "ws://sala.test/ws";
const PASSWORD = "123456";
const SETTINGS = { minHeight: 360, maxHeight: 1080, minFps: 15, maxFps: 60 };

type Info = { id: string; name: string; sharing: boolean; label: string | null };

/** Imita o server.rs: senha, welcome, peer-joined/left/updated e repasse de signal. */
class FakeServer {
  peers = new Map<FakeWS, Info>();
  private next = 1;

  receive(ws: FakeWS, msg: any) {
    const me = this.peers.get(ws);
    if (msg.type === "join" && !me) {
      if (msg.password.trim() !== PASSWORD) {
        ws.deliver({ type: "error", reason: "wrong-password" });
        ws.serverClose();
        return;
      }
      const info: Info = { id: String(this.next++), name: msg.name, sharing: false, label: null };
      ws.deliver({ type: "welcome", id: info.id, peers: [...this.peers.values()], settings: SETTINGS });
      this.broadcast({ type: "peer-joined", peer: info });
      this.peers.set(ws, info);
    } else if (msg.type === "sharing" && me) {
      me.sharing = msg.sharing;
      me.label = msg.sharing ? (msg.label ?? null) : null;
      this.broadcast({ type: "peer-updated", id: me.id, sharing: me.sharing, label: me.label });
    } else if (msg.type === "signal" && me) {
      for (const [other, info] of this.peers) {
        if (info.id === msg.to) other.deliver({ type: "signal", from: me.id, data: msg.data });
      }
    }
  }

  drop(ws: FakeWS) {
    const info = this.peers.get(ws);
    if (!info) return;
    this.peers.delete(ws);
    this.broadcast({ type: "peer-left", id: info.id });
  }

  private broadcast(msg: unknown) {
    for (const ws of this.peers.keys()) ws.deliver(msg);
  }
}

let server: FakeServer;

class FakeWS {
  static OPEN = 1;
  readyState = 0;
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor(public url: string) {
    setTimeout(() => {
      this.readyState = 1;
      this.onopen?.();
    });
  }
  send(text: string) {
    const msg = JSON.parse(text);
    setTimeout(() => server.receive(this, msg));
  }
  deliver(msg: unknown) {
    setTimeout(() => this.readyState === 1 && this.onmessage?.({ data: JSON.stringify(msg) }));
  }
  /** O servidor fechou (depois de entregar o que já estava na fila). */
  serverClose() {
    setTimeout(() => this.close());
  }
  close() {
    if (this.readyState === 3) return;
    this.readyState = 3;
    server.drop(this);
    setTimeout(() => this.onclose?.());
  }
}

function capture(...kinds: string[]): NativeCapture {
  return { stream: stream(...kinds), stop: vi.fn(), onEnded: null, frameHeight: 1080 };
}

async function join(name: string) {
  const client = new RoomClient(URL, name, PASSWORD);
  await client.connect();
  await settle();
  return client;
}

beforeEach(() => {
  server = new FakeServer();
  installFakeRtc();
  vi.stubGlobal("WebSocket", FakeWS);
  vi.stubGlobal("document", {
    hidden: false,
    addEventListener: () => {},
    removeEventListener: () => {},
    // snapshot(): o vídeo falso não toca, então a prévia sai vazia
    createElement: () => ({ play: () => Promise.reject(new Error("sem vídeo")), pause() {} }),
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("RoomClient", () => {
  it("entra na sala e vê quem já estava e quem chega", async () => {
    const ana = await join("Ana");
    const events: string[] = [];
    ana.onPeerEvent = (kind) => events.push(kind);
    const bia = await join("Bia");

    expect(ana.status).toBe("connected");
    expect(bia.settings).toEqual(SETTINGS);
    expect([...ana.peers.values()].map((p) => p.name).sort()).toEqual(["Ana", "Bia"]);
    expect([...bia.peers.values()].map((p) => p.name).sort()).toEqual(["Ana", "Bia"]);
    expect(events).toEqual(["join"]);
  });

  it("senha errada vira WrongPasswordError", async () => {
    const intruso = new RoomClient(URL, "Intruso", "000000");
    await expect(intruso.connect()).rejects.toBeInstanceOf(WrongPasswordError);
  });

  it("transmitir, assistir, receber o vídeo e parar", async () => {
    const ana = await join("Ana");
    const bia = await join("Bia");
    const events: string[] = [];
    bia.onPeerEvent = (kind) => events.push(kind);

    const cap = capture("video", "audio");
    ana.startShare(cap, { height: 1080, fps: 60 }, "Jogo", true);
    await settle();
    expect(bia.peers.get(ana.selfId)).toMatchObject({ sharing: true, label: "1080p 60fps · Jogo" });
    expect(events).toEqual(["live"]);

    bia.watch(ana.selfId);
    expect(bia.views.get(ana.selfId)?.state).toBe("connecting");
    await settle();

    const [sharerPc, viewerPc] = FakePC.all;
    expect(sharerPc.senders.map((s) => s.track.kind)).toEqual(["video", "audio"]);
    viewerPc.ontrack!({ streams: [cap.stream], track: sharerPc.senders[0].track });
    viewerPc.setState("connected");
    expect(bia.views.get(ana.selfId)).toEqual({ stream: cap.stream, state: "connected" });

    // tile pequeno: a Ana passa a mandar menos
    bia.setViewSize(ana.selfId, 640, 360);
    await settle();
    expect(sharerPc.senders[0].params.encodings[0]).toMatchObject({ active: true, scaleResolutionDownBy: 3 });

    ana.stopShare();
    await settle();
    expect(cap.stop).toHaveBeenCalled();
    expect(sharerPc.closed).toBe(true);
    expect(viewerPc.closed).toBe(true);
    expect(bia.watching.size).toBe(0);
    expect(bia.views.size).toBe(0);
  });

  it("trocar de tela mantém quem está assistindo", async () => {
    const ana = await join("Ana");
    const bia = await join("Bia");
    const first = capture("video", "audio");
    ana.startShare(first, { height: 720, fps: 30 }, "Jogo", true);
    await settle();
    bia.watch(ana.selfId);
    await settle();
    const [sharerPc] = FakePC.all;

    ana.releaseCapture();
    const second = capture("video", "audio");
    ana.switchShare(second, { height: 720, fps: 30 }, "Navegador", true);
    await settle();
    expect(first.stop).toHaveBeenCalled();
    expect(sharerPc.closed).toBe(false);
    expect(sharerPc.senders.map((s) => s.track)).toEqual(second.stream.getTracks());
    expect(bia.peers.get(ana.selfId)?.label).toBe("720p 30fps · Navegador");
  });

  it("pede a prévia de quem está ao vivo", async () => {
    const ana = await join("Ana");
    const bia = await join("Bia");
    ana.startShare(capture("video"), { height: 720, fps: 30 }, "Jogo", true);
    await settle();

    bia.requestThumb(ana.selfId);
    expect(bia.isThumbLoading(ana.selfId)).toBe(true);
    await settle();
    expect(bia.isThumbLoading(ana.selfId)).toBe(false);
    expect(bia.thumbs.get(ana.selfId)?.image).toBeNull();
  });

  it("quem sai some da sala e da tela de quem assistia", async () => {
    const ana = await join("Ana");
    const bia = await join("Bia");
    ana.startShare(capture("video"), { height: 720, fps: 30 }, "Jogo", true);
    await settle();
    bia.watch(ana.selfId);
    await settle();
    const events: string[] = [];
    bia.onPeerEvent = (kind) => events.push(kind);

    ana.leave();
    await settle();
    expect(ana.status).toBe("closed");
    expect(bia.peers.has(ana.selfId)).toBe(false);
    expect(bia.views.size).toBe(0);
    expect(FakePC.all.every((pc) => pc.closed)).toBe(true);
    expect(events).toEqual(["leave"]);
  });
});
