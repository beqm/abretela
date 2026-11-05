// Textos dos logs de rede: rotas, NAT, SDP e o palpite de por que falhou.

import { describe, expect, it } from "vitest";
import { describeNat, describeRoute, describeSdp, explainFailure, formatElapsed, parseCandidate } from "./routes";

const srflx = (pub: string, port: number, base = "0.0.0.0", basePort = 0) =>
  parseCandidate(`candidate:1 1 udp 1677729535 ${pub} ${port} typ srflx raddr ${base} rport ${basePort} generation 0`)!;

describe("rotas", () => {
  it("lê uma rota pública com o endereço local por trás", () => {
    const r = srflx("189.1.2.3", 54321, "192.168.0.10", 50000);
    expect(r).toMatchObject({ type: "srflx", protocol: "udp", address: "189.1.2.3", port: 54321 });
    expect(describeRoute(r)).toBe(
      "public address (STUN) 189.1.2.3:54321 over UDP (IPv4, behind 192.168.0.10:50000)",
    );
  });

  it("aceita a linha com 'a=' e rotas escondidas pelo navegador (mDNS)", () => {
    const r = parseCandidate("a=candidate:2 1 udp 2122260223 abc-123.local 60000 typ host generation 0")!;
    expect(describeRoute(r)).toBe("local network abc-123.local:60000 over UDP (address hidden by the browser)");
  });

  it("marca CGNAT, Radmin/Hamachi e TCP", () => {
    const tcp = parseCandidate("candidate:3 1 tcp 1518280447 26.10.0.5 9 typ host tcptype active")!;
    expect(describeRoute(tcp)).toContain("Radmin VPN / Hamachi");
    expect(describeRoute(tcp)).toContain("TCP active");
    expect(describeRoute(srflx("100.70.1.1", 1))).toContain("CGNAT");
  });

  it("ignora linhas que não são rotas", () => {
    expect(parseCandidate("")).toBeNull();
    expect(parseCandidate("candidate:1 1 udp")).toBeNull();
  });
});

describe("NAT", () => {
  it("porta pública que muda por servidor = NAT simétrico", () => {
    const nat = describeNat([srflx("189.1.2.3", 1000, "192.168.0.10", 5000), srflx("189.1.2.3", 1001, "192.168.0.10", 5000)]);
    expect(nat).toContain("DIFFERENT public port");
    expect(nat).not.toContain("probably");
  });

  it("sem o endereço local, ainda dá um palpite", () => {
    expect(describeNat([srflx("189.1.2.3", 1000), srflx("189.1.2.3", 1001)])).toContain("probably strict");
  });

  it("mesma porta para todos = NAT amigável", () => {
    expect(describeNat([srflx("189.1.2.3", 1000, "192.168.0.10", 5000), srflx("189.1.2.3", 1000, "192.168.0.10", 5000)])).toContain(
      "friendly NAT",
    );
  });

  it("sem rota pública: STUN não respondeu", () => {
    const host = parseCandidate("candidate:2 1 udp 1 192.168.0.10 60000 typ host")!;
    expect(describeNat([host])).toContain("STUN servers did not answer");
    expect(describeNat([])).toBe("no routes at all");
  });
});

describe("SDP", () => {
  it("resume codecs, perfil do H.264, papel do DTLS e rotas", () => {
    const sdp = [
      "v=0",
      "a=fingerprint:sha-256 AA:BB",
      "a=setup:actpass",
      "m=video 9 UDP/TLS/RTP/SAVPF 102 103 96",
      "a=sendonly",
      "a=rtpmap:102 H264/90000",
      "a=fmtp:102 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f",
      "a=rtpmap:103 rtx/90000",
      "a=rtpmap:96 VP8/90000",
      "a=candidate:1 1 udp 1 1.2.3.4 5 typ host",
      "m=audio 0 UDP/TLS/RTP/SAVPF 111",
      "a=rtpmap:111 opus/48000/2",
      "",
    ].join("\r\n");
    expect(describeSdp(sdp)).toBe(
      "video sendonly: H264 (profile 42e01f), also VP8; audio rejected [DTLS role actpass, fingerprint sha-256, 1 routes inside]",
    );
  });
});

describe("motivo da falha", () => {
  const base = { localRoutes: 3, remoteRoutes: 3 };

  it("nada passa em nenhum sentido", () => {
    const pairs = [{ state: "failed", requestsSent: 10, responsesReceived: 0, requestsReceived: 0 }];
    expect(explainFailure({ ...base, pairs })).toContain("blocked in both directions");
  });

  it("chega daqui, não volta", () => {
    const pairs = [{ state: "failed", requestsSent: 10, responsesReceived: 0, requestsReceived: 4 }];
    expect(explainFailure({ ...base, pairs })).toContain("not reaching them");
  });

  it("rota funcionou mas o DTLS falhou", () => {
    const pairs = [{ state: "succeeded", requestsSent: 3, responsesReceived: 3 }];
    expect(explainFailure({ ...base, pairs, dtlsState: "failed" })).toContain("encryption handshake (DTLS) failed");
    expect(explainFailure({ ...base, pairs, dtlsState: "connecting" })).toContain('stuck at "connecting"');
  });

  it("o outro lado não mandou rotas", () => {
    expect(explainFailure({ localRoutes: 2, remoteRoutes: 0, pairs: [] })).toContain("check their log");
  });
});

it("formata o tempo", () => {
  expect(formatElapsed(1400)).toBe("1.4s");
  expect(formatElapsed(133_000)).toBe("2m 13s");
  expect(formatElapsed(3_900_000)).toBe("1h 5m");
});
