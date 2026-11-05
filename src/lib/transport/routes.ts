// Textos dos logs de rede: transforma rotas ICE, SDP e estatísticas do WebRTC
// em frases legíveis e tenta explicar por que uma conexão falhou. Tudo aqui é
// puro (sem WebRTC), para dar para testar.

export const ROUTE_NAMES: Record<string, string> = {
  host: "local network",
  srflx: "public address (STUN)",
  prflx: "public address (discovered)",
  relay: "relay (TURN)",
};

export const routeName = (type: string | undefined) => (type ? ROUTE_NAMES[type] ?? type : "unknown");

/** Ex.: "2 local network, 1 public address (STUN)". */
export function summarizeRoutes(types: string[]): string {
  if (!types.length) return "none";
  const counts = new Map<string, number>();
  types.forEach((t) => counts.set(t, (counts.get(t) ?? 0) + 1));
  return [...counts].map(([t, n]) => `${n} ${routeName(t)}`).join(", ");
}

/** "1.4s" ou "2m 13s". */
export function formatElapsed(ms: number): string {
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  const s = Math.round(ms / 1000);
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}

// ---------- rotas (candidatos ICE) ----------

export type Route = {
  type: string;
  protocol: string;
  address: string;
  port: number;
  /** Endereço local por trás do público (srflx/relay), se o navegador mostrar. */
  relatedAddress?: string;
  relatedPort?: number;
  tcpType?: string;
};

/** Lê uma linha "candidate:..." do SDP. Devolve `null` se não der para entender. */
export function parseCandidate(line: string | undefined): Route | null {
  if (!line) return null;
  const parts = line.replace(/^a=/, "").trim().split(/\s+/);
  if (parts.length < 8 || parts[6] !== "typ") return null;
  const route: Route = {
    protocol: parts[2].toLowerCase(),
    address: parts[4],
    port: Number(parts[5]),
    type: parts[7],
  };
  for (let i = 8; i + 1 < parts.length; i += 2) {
    const [key, value] = [parts[i], parts[i + 1]];
    if (key === "raddr") route.relatedAddress = value;
    if (key === "rport") route.relatedPort = Number(value);
    if (key === "tcptype") route.tcpType = value;
  }
  return route;
}

const isHidden = (address: string) => address.endsWith(".local");
const isUnset = (address?: string) => !address || address === "0.0.0.0" || address === "::";

function addressKind(address: string): string {
  if (isHidden(address)) return "address hidden by the browser";
  if (address.includes(":")) return "IPv6";
  if (/^(10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(address)) return "IPv4, private";
  if (/^100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\./.test(address)) return "IPv4, carrier NAT range (CGNAT)";
  if (/^169\.254\./.test(address)) return "IPv4, self-assigned (no DHCP)";
  if (/^(26\.|25\.)/.test(address)) return "IPv4, usual range of Radmin VPN / Hamachi";
  return "IPv4";
}

/** Ex.: "public address (STUN) 189.1.2.3:54321 over UDP (IPv4, behind 192.168.0.10:54321)". */
export function describeRoute(r: Route): string {
  const extras = [addressKind(r.address)];
  if (!isUnset(r.relatedAddress)) extras.push(`behind ${r.relatedAddress}:${r.relatedPort}`);
  if (r.tcpType) extras.push(`TCP ${r.tcpType}`);
  return `${routeName(r.type)} ${r.address}:${r.port} over ${r.protocol.toUpperCase()} (${extras.join(", ")})`;
}

/**
 * Olha as rotas públicas (STUN) de um PC e diz como o roteador dele se
 * comporta. Se a mesma porta local aparece com portas públicas diferentes
 * para servidores diferentes, o roteador troca a porta por destino (NAT
 * simétrico) e a conexão direta costuma falhar.
 */
export function describeNat(routes: Route[]): string {
  const stun = routes.filter((r) => r.type === "srflx" && r.protocol === "udp");
  if (!routes.length) return "no routes at all";
  if (!stun.length) {
    return "no public address found (the STUN servers did not answer: UDP may be blocked on this network)";
  }
  const publicIps = [...new Set(stun.map((r) => r.address))];
  const byBase = new Map<string, Set<number>>();
  let knownBase = true;
  for (const r of stun) {
    if (isUnset(r.relatedAddress)) knownBase = false;
    const base = knownBase ? `${r.relatedAddress}:${r.relatedPort}` : r.address;
    if (!byBase.has(base)) byBase.set(base, new Set());
    byBase.get(base)!.add(r.port);
  }
  const ips = `public IP ${publicIps.join(" and ")}`;
  const changing = [...byBase.values()].some((ports) => ports.size > 1);
  if (changing) {
    return (
      `${ips}; the router gives a DIFFERENT public port to each server ` +
      `(${knownBase ? "" : "probably "}strict/symmetric NAT: direct connections often fail with it)`
    );
  }
  const cgnat = publicIps.some((ip) => /^100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\./.test(ip));
  if (cgnat) return `${ips}, which is inside the carrier NAT range (CGNAT): the internet provider shares this IP`;
  if (stun.length === 1) return `${ips}; only one STUN server answered, so the NAT type is unknown`;
  return `${ips}; the router keeps the same public port for every server (friendly NAT)`;
}

// ---------- SDP ----------

/** Resumo de uma oferta/resposta: o que cada lado aceitou. */
export function describeSdp(sdp: string): string {
  const sections = sdp.split(/\r?\nm=/).slice(1);
  const parts: string[] = [];
  for (const section of sections) {
    const kind = section.split(" ")[0];
    const direction = section.match(/a=(sendrecv|sendonly|recvonly|inactive)/)?.[1] ?? "?";
    const port = section.split(" ")[1];
    if (port === "0") {
      parts.push(`${kind} rejected`);
      continue;
    }
    const payloads = section.split(/\r?\n/)[0].split(" ").slice(3);
    const names = new Map<string, string>();
    for (const m of section.matchAll(/a=rtpmap:(\d+) ([\w-]+)/g)) names.set(m[1], m[2]);
    const codecs = payloads
      .map((p) => names.get(p))
      .filter((c): c is string => !!c && !/^(rtx|red|ulpfec|flexfec-03|telephone-event|CN)$/i.test(c));
    const unique = [...new Set(codecs)];
    let first = unique[0] ?? "no codec";
    if (first === "H264") {
      const pt = payloads.find((p) => names.get(p) === "H264");
      const profile = section.match(new RegExp(`a=fmtp:${pt} .*profile-level-id=([0-9a-fA-F]{6})`))?.[1];
      if (profile) first += ` (profile ${profile})`;
    }
    const others = unique.length > 1 ? `, also ${unique.slice(1).join("/")}` : "";
    parts.push(`${kind} ${direction}: ${first}${others}`);
  }
  const setup = sdp.match(/a=setup:(\w+)/)?.[1];
  const fingerprint = sdp.match(/a=fingerprint:([\w-]+)/)?.[1];
  const candidates = (sdp.match(/a=candidate:/g) ?? []).length;
  const extra = [
    setup && `DTLS role ${setup}`,
    fingerprint && `fingerprint ${fingerprint}`,
    candidates && `${candidates} routes inside`,
  ].filter(Boolean);
  return `${parts.join("; ") || "no media"}${extra.length ? ` [${extra.join(", ")}]` : ""}`;
}

// ---------- por que falhou ----------

export type PairInfo = {
  state: string;
  nominated?: boolean;
  requestsSent?: number;
  responsesReceived?: number;
  requestsReceived?: number;
  responsesSent?: number;
};

export type FailureFacts = {
  localRoutes: number;
  remoteRoutes: number;
  pairs: PairInfo[];
  iceState?: string;
  dtlsState?: string;
};

/** Uma frase com o motivo mais provável da falha, a partir das estatísticas. */
export function explainFailure(f: FailureFacts): string {
  if (!f.localRoutes) {
    return "This PC found no network routes at all: no usable network adapter, or WebRTC is blocked on this PC.";
  }
  if (!f.remoteRoutes) {
    return (
      "The other PC never sent any network route. The problem is on their side " +
      "(or the room messages are not arriving): check their log."
    );
  }
  const succeeded = f.pairs.some((p) => p.state === "succeeded");
  if (succeeded && f.dtlsState === "failed") {
    return (
      "The network path WORKS, but the encryption handshake (DTLS) failed. Something on one of the PCs " +
      "(antivirus/firewall inspecting traffic, a VPN, or a broken WebView2) is interfering, not the router."
    );
  }
  if (succeeded && f.dtlsState && f.dtlsState !== "connected") {
    return (
      `The network path works, but the encryption handshake (DTLS) never finished (stuck at "${f.dtlsState}"). ` +
      "Packets get through only partly: suspect an antivirus, a VPN or packet size (MTU) problems."
    );
  }
  if (succeeded) return "A route worked at some point, but the connection was lost later (network dropped or changed).";
  if (!f.pairs.length) return "No route pairs were ever tried: the routes may have arrived too late or were invalid.";
  const sum = (k: keyof PairInfo) => f.pairs.reduce((n, p) => n + (Number(p[k]) || 0), 0);
  const sent = sum("requestsSent");
  const answered = sum("responsesReceived");
  const received = sum("requestsReceived");
  if (!sent) return "This PC never sent a connection check (the routes may have arrived too late or were invalid).";
  if (!answered && !received) {
    return (
      `This PC sent ${sent} connection checks and none were answered, and none arrived from the other PC. ` +
      "Packets are blocked in both directions: strict NAT/CGNAT on both sides, a firewall or a VPN. " +
      "A relay (TURN) or another participant relaying would be needed."
    );
  }
  if (received && !answered) {
    return (
      `Checks from the other PC reach this PC (${received} arrived), but the ${sent} checks sent from here ` +
      "never got an answer: traffic from this PC is not reaching them (their router/NAT or firewall drops it)."
    );
  }
  if (answered && !received) {
    return (
      `This PC's checks were answered (${answered} of ${sent}), but no check came from the other PC and no ` +
      "route was confirmed: the connection works one way only."
    );
  }
  return `Checks went both ways (${sent} sent, ${answered} answered, ${received} received) but no route was confirmed.`;
}
