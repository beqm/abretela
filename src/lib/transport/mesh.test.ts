// O transporte em malha com um RTCPeerConnection falso: confere as mensagens
// trocadas entre quem transmite e quem assiste, sem rede nem navegador.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MeshTransport } from "./mesh";
import type { Publication, RemoteView, TransportSignal } from "./types";
import { FakePC, installFakeRtc, stream, track } from "../testing/fakeRtc";

vi.mock("../log", () => ({
  log: { info: () => {}, warn: () => {}, error: () => {} },
  describeError: (e: unknown) => String(e),
}));

/** Duas (ou mais) pontas ligadas por uma fila de mensagens. */
function network() {
  const queue: { from: string; to: string; data: TransportSignal }[] = [];
  const sent: typeof queue = [];
  const nodes = new Map<string, MeshTransport>();
  const remotes = new Map<string, Partial<RemoteView>>();
  const add = (id: string) => {
    const t = new MeshTransport({
      signal: (to, data) => {
        queue.push({ from: id, to, data });
        sent.push({ from: id, to, data });
      },
      nameOf: (x) => `"${x}"`,
      onRemote: (from, patch) => remotes.set(`${id}<-${from}`, { ...remotes.get(`${id}<-${from}`), ...patch }),
    });
    nodes.set(id, t);
    return t;
  };
  /** Entrega tudo o que está na fila (e o que surgir), deixando as promessas andarem. */
  const pump = async () => {
    for (let i = 0; i < 50; i++) {
      await new Promise((r) => setTimeout(r, 0));
      const batch = queue.splice(0);
      if (!batch.length && i > 2) return;
      for (const m of batch) nodes.get(m.to)?.handleSignal(m.from, m.data);
    }
  };
  return { add, pump, sent, remotes };
}

const pub = (s: MediaStream, height = 1080, fps = 60): Publication => ({
  stream: s,
  quality: { height, fps },
  hwEncode: true,
  sourceHeight: () => height,
});

beforeEach(() => {
  installFakeRtc();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("MeshTransport", () => {
  it("assistir: watch -> offer -> answer e o vídeo chega para quem assiste", async () => {
    const net = network();
    const ana = net.add("ana");
    const bia = net.add("bia");
    const screen = stream("video", "audio");
    ana.publish(pub(screen));

    bia.watch("ana");
    await net.pump();

    const kinds = net.sent.map((m) => `${m.from}:${m.data.kind}`);
    expect(kinds.slice(0, 3)).toEqual(["bia:watch", "ana:offer", "bia:answer"]);
    const [sharerPc, viewerPc] = FakePC.all;
    expect(sharerPc.senders.map((s) => s.track.kind)).toEqual(["video", "audio"]);
    expect(sharerPc.remoteDescription?.type).toBe("answer");
    expect(viewerPc.remoteDescription?.type).toBe("offer");

    viewerPc.ontrack!({ streams: [screen], track: track("video") });
    expect(net.remotes.get("bia<-ana")).toEqual({ stream: screen });
  });

  it("ignora oferta de quem eu não pedi para assistir", async () => {
    const bia = new MeshTransport({ signal: () => {}, nameOf: (x) => x, onRemote: () => {} });
    bia.handleSignal("estranho", { kind: "offer", sdp: "v=0" });
    expect(FakePC.all).toHaveLength(0);
  });

  it("quem não está transmitindo não responde ao watch", async () => {
    const net = network();
    net.add("ana");
    const bia = net.add("bia");
    bia.watch("ana");
    await net.pump();
    expect(net.sent.map((m) => m.data.kind)).toEqual(["watch"]);
  });

  it("ajusta o vídeo ao tamanho que cada um está vendo", async () => {
    const net = network();
    const ana = net.add("ana");
    const bia = net.add("bia");
    ana.publish(pub(stream("video"), 1080, 60));
    bia.watch("ana");
    await net.pump();
    const video = () => FakePC.all[0].senders[0].params.encodings[0];

    bia.requestView("ana", 360); // miniatura
    await net.pump();
    expect(video()).toMatchObject({ active: true, scaleResolutionDownBy: 3, maxFramerate: 30 });

    bia.requestView("ana", 360); // igual: não manda de novo
    bia.requestView("ana", 0); // app escondido
    await net.pump();
    expect(net.sent.filter((m) => m.data.kind === "view")).toHaveLength(2);
    expect(video().active).toBe(false);
  });

  it("trocar de tela com as mesmas faixas só troca o conteúdo", async () => {
    const net = network();
    const ana = net.add("ana");
    const bia = net.add("bia");
    ana.publish(pub(stream("video", "audio")));
    bia.watch("ana");
    await net.pump();

    const next = stream("video", "audio");
    expect(ana.replace(pub(next))).toBe(true);
    await net.pump();
    expect(FakePC.all[0].closed).toBe(false);
    expect(FakePC.all[0].senders.map((s) => s.track)).toEqual(next.getTracks());
  });

  it("trocar para uma tela com outras faixas reconecta cada espectador uma vez", async () => {
    const net = network();
    const ana = net.add("ana");
    const bia = net.add("bia");
    const caio = net.add("caio");
    ana.publish(pub(stream("video", "audio")));
    bia.watch("ana");
    caio.watch("ana");
    await net.pump();
    const offersBefore = net.sent.filter((m) => m.data.kind === "offer").length;

    // antes, o laço visitava de novo a conexão recriada e não terminava
    expect(ana.replace(pub(stream("video")))).toBe(false);
    await net.pump();
    const offers = net.sent.filter((m) => m.data.kind === "offer").slice(offersBefore);
    expect(offers.map((m) => m.to).sort()).toEqual(["bia", "caio"]);
  });

  it("parar de transmitir e sair fecham as conexões", async () => {
    const net = network();
    const ana = net.add("ana");
    const bia = net.add("bia");
    ana.publish(pub(stream("video")));
    bia.watch("ana");
    await net.pump();
    const [sharerPc, viewerPc] = FakePC.all;

    bia.stopWatching("ana", true);
    expect(viewerPc.closed).toBe(true);
    await net.pump();
    expect(sharerPc.closed).toBe(true);
    expect(net.sent.at(-1)?.data.kind).toBe("unwatch");
  });

  it("explica onde travou quem não recebeu oferta", () => {
    const bia = new MeshTransport({ signal: () => {}, nameOf: (x) => x, onRemote: () => {} });
    bia.watch("ana");
    expect(bia.describeStuck("ana")).toMatch(/never sent an offer/);
  });
});
