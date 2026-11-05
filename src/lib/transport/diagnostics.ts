// Logs das conexões WebRTC, o mais detalhados possível: cada rota de rede
// achada (com IP e porta), cada etapa (mensagens, checagem de rotas, ICE, e
// a criptografia, DTLS), qual rota foi escolhida, a saúde do vídeo enquanto
// está conectado e, quando falha, todos os pares de rota testados e um palpite
// do motivo. Os textos ficam em `routes.ts`.

import { describeError, log } from "../log";
import {
  describeNat,
  describeRoute,
  explainFailure,
  formatElapsed,
  parseCandidate,
  routeName,
  summarizeRoutes,
  type PairInfo,
  type Route,
} from "./routes";

export { summarizeRoutes } from "./routes";

/** Uma conexão com alguém, com o que precisa para os logs. */
export type Instrumented = {
  pc: RTCPeerConnection;
  /** "sharing" (eu transmito) ou "watching" (eu assisto). */
  area: "sharing" | "watching";
  /** Ex.: 'to "Ana"' ou 'from "Ana"'. */
  who: string;
  /** Rotas achadas aqui e recebidas do outro lado. */
  localRoutes: Route[];
  remoteRoutes: Route[];
  /** Quando a conexão foi criada (Date.now()). */
  startedAt: number;
};

/** De quanto tempo para cá, para os logs: "(+1.4s)". */
const since = (conn: Instrumented) => `(+${formatElapsed(Date.now() - conn.startedAt)})`;

/** Quantas vezes a saúde do vídeo é escrita no log enquanto conectado. */
const HEALTH_EVERY_MS = 30_000;
/** Limite de linhas de rota/erro por conexão (para não inundar o log). */
const MAX_ROUTE_LINES = 20;
const MAX_CANDIDATE_ERRORS = 10;
const MAX_PAIR_LINES = 15;

const STUN_ERRORS: Record<number, string> = {
  701: "no answer from the server (blocked, offline or this network has no route to it)",
  300: "try another server",
  400: "bad request",
  401: "unauthorized",
  403: "forbidden",
  437: "allocation mismatch",
  438: "stale nonce",
  486: "allocation quota reached",
  500: "server error",
  508: "insufficient capacity",
};

// ---------- rotas ----------

/** Uma rota achada neste PC (evento `icecandidate`). */
export function noteLocalRoute(conn: Instrumented, candidate: RTCIceCandidate) {
  const route = parseCandidate(candidate.candidate) ?? {
    type: candidate.type ?? "unknown",
    protocol: candidate.protocol ?? "?",
    address: candidate.address ?? "?",
    port: candidate.port ?? 0,
  };
  conn.localRoutes.push(route);
  if (conn.localRoutes.length <= MAX_ROUTE_LINES) {
    log.info(conn.area, `Route #${conn.localRoutes.length} on this PC ${conn.who}: ${describeRoute(route)} ${since(conn)}`);
  } else if (conn.localRoutes.length === MAX_ROUTE_LINES + 1) {
    log.info(conn.area, `More routes on this PC ${conn.who}, not listing them one by one`);
  }
}

/** Uma rota que o outro PC mandou. */
export function noteRemoteRoute(conn: Instrumented, candidate: RTCIceCandidateInit) {
  const line = candidate.candidate;
  if (!line) {
    log.info(conn.area, `The other PC finished sending routes ${conn.who} ${since(conn)}`);
    return;
  }
  const route = parseCandidate(line);
  if (!route) {
    log.warn(conn.area, `Got a route ${conn.who} that could not be read: "${line}"`);
    return;
  }
  conn.remoteRoutes.push(route);
  if (conn.remoteRoutes.length <= MAX_ROUTE_LINES) {
    log.info(
      conn.area,
      `Route #${conn.remoteRoutes.length} from the other PC ${conn.who}: ${describeRoute(route)} ${since(conn)}`,
    );
  }
}

// ---------- estatísticas ----------

type Stats = Map<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

function routeFromStats(s: any): Route | null {
  if (!s) return null;
  return {
    type: s.candidateType,
    protocol: s.protocol ?? "?",
    address: s.address ?? s.ip ?? "?",
    port: s.port ?? 0,
    relatedAddress: s.relatedAddress,
    relatedPort: s.relatedPort,
    tcpType: s.tcpType,
  };
}

function describeStatsRoute(s: any): string {
  const route = routeFromStats(s);
  if (!route) return "unknown route";
  const network = s.networkType && s.networkType !== "unknown" ? `, ${s.networkType}` : "";
  const vpn = s.vpn ? ", through a VPN adapter" : "";
  return `${describeRoute(route)}${network}${vpn}`;
}

const ms = (seconds: number | undefined) => (seconds == null ? "?" : `${Math.round(seconds * 1000)} ms`);
const kbps = (bits: number | undefined) => (bits == null ? "?" : `${Math.round(bits / 1000)} kbps`);

function selectedPair(stats: Stats) {
  const all = [...stats.values()];
  const transport = all.find((s) => s.type === "transport");
  return (
    (transport?.selectedCandidatePairId && stats.get(transport.selectedCandidatePairId)) ||
    all.find((s) => s.type === "candidate-pair" && s.nominated && s.state === "succeeded")
  );
}

async function getStats(conn: Instrumented): Promise<Stats | null> {
  try {
    return (await conn.pc.getStats()) as unknown as Stats;
  } catch (e) {
    log.warn(conn.area, `Could not read the connection statistics ${conn.who}: ${describeError(e)}`);
    return null;
  }
}

/**
 * Escreve tudo o que se sabe da conexão: etapas, todos os pares de rota
 * testados (pedidos e respostas) e um palpite do motivo da falha.
 */
export async function logConnectionReport(conn: Instrumented, title: string, level: "warn" | "error" = "error") {
  const { pc, area, who } = conn;
  const stats = await getStats(conn);
  const all = stats ? [...stats.values()] : [];
  const transport = all.find((s) => s.type === "transport");
  const dtlsState = transport?.dtlsState ?? dtlsOf(pc)?.state;
  log[level](
    area,
    `${title} ${who} ${since(conn)}. Connection "${pc.connectionState}", route checks (ICE) "${pc.iceConnectionState}", ` +
      `encryption (DTLS) "${dtlsState ?? "?"}", route gathering "${pc.iceGatheringState}", messages "${pc.signalingState}"`,
  );
  log[level](area, `  Routes on this PC: ${summarizeRoutes(conn.localRoutes.map((r) => r.type))}. Router here: ${describeNat(conn.localRoutes)}`);
  log[level](
    area,
    `  Routes from the other PC: ${summarizeRoutes(conn.remoteRoutes.map((r) => r.type))}. ` +
      `Router there: ${describeNat(conn.remoteRoutes)}`,
  );

  const pairs = all.filter((s) => s.type === "candidate-pair");
  // os mais interessantes primeiro: os que trocaram alguma coisa
  const activity = (p: any) => (p.requestsSent ?? 0) + (p.responsesReceived ?? 0) + (p.requestsReceived ?? 0);
  pairs.sort((a, b) => Number(b.state === "succeeded") - Number(a.state === "succeeded") || activity(b) - activity(a));
  log[level](area, `  ${pairs.length} route pairs were tried:`);
  for (const p of pairs.slice(0, MAX_PAIR_LINES)) {
    const local = describeStatsRoute(stats!.get(p.localCandidateId));
    const remote = describeStatsRoute(stats!.get(p.remoteCandidateId));
    log[level](
      area,
      `  - ${local} -> ${remote}: ${p.state}${p.nominated ? " (chosen)" : ""}; ` +
        `checks sent ${p.requestsSent ?? 0}, answered ${p.responsesReceived ?? 0}; ` +
        `checks received ${p.requestsReceived ?? 0}, answered ${p.responsesSent ?? 0}` +
        (p.currentRoundTripTime != null ? `; round trip ${ms(p.currentRoundTripTime)}` : "") +
        (p.bytesSent || p.bytesReceived
          ? `; ${Math.round((p.bytesSent ?? 0) / 1024)} KB sent, ${Math.round((p.bytesReceived ?? 0) / 1024)} KB received`
          : ""),
    );
  }
  if (pairs.length > MAX_PAIR_LINES) log[level](area, `  ... and ${pairs.length - MAX_PAIR_LINES} more pairs`);

  const facts = {
    localRoutes: conn.localRoutes.length,
    remoteRoutes: conn.remoteRoutes.length,
    pairs: pairs as PairInfo[],
    iceState: transport?.iceState ?? pc.iceConnectionState,
    dtlsState,
  };
  log[level](area, `  Most likely reason: ${explainFailure(facts)}`);
}

/** Por qual rota conectou, com atraso e criptografia. */
async function describeConnected(conn: Instrumented) {
  const stats = await getStats(conn);
  if (!stats) return;
  const pair = selectedPair(stats);
  const transport = [...stats.values()].find((s) => s.type === "transport");
  if (!pair) {
    log.info(conn.area, `Connected ${conn.who} ${since(conn)}, but the chosen route is unknown`);
    return;
  }
  const local = stats.get(pair.localCandidateId);
  const remote = stats.get(pair.remoteCandidateId);
  log.info(
    conn.area,
    `Connected ${conn.who} ${since(conn)} via ${routeName(local?.candidateType)} -> ${routeName(remote?.candidateType)}. ` +
      `This PC: ${describeStatsRoute(local)}. Other PC: ${describeStatsRoute(remote)}. ` +
      `Round trip ${ms(pair.currentRoundTripTime)}, estimated upload ${kbps(pair.availableOutgoingBitrate)}`,
  );
  if (transport) {
    log.info(
      conn.area,
      `Encryption ${conn.who}: ${transport.tlsVersion ? `DTLS ${transport.tlsVersion}` : "DTLS"}, ` +
        `cipher ${transport.dtlsCipher ?? "?"}, media cipher ${transport.srtpCipher ?? "?"}, ` +
        `this PC is the DTLS ${transport.dtlsRole ?? "?"}`,
    );
  }
}

/** Alguns segundos depois de conectar: o vídeo está mesmo saindo/chegando? */
async function describeVideo(conn: Instrumented) {
  if (conn.pc.connectionState !== "connected") return;
  const stats = await getStats(conn);
  stats?.forEach((s) => {
    if (s.kind !== "video") return;
    if (s.type === "outbound-rtp") {
      const codec = stats.get(s.codecId)?.mimeType?.replace("video/", "") ?? "?";
      const msg =
        `Video ${conn.who} after 5s: ${codec}, ${s.framesEncoded ?? 0} frames encoded, ` +
        `${s.frameWidth ?? "?"}x${s.frameHeight ?? "?"} at ${s.framesPerSecond ?? 0} fps, ` +
        `${Math.round((s.bytesSent ?? 0) / 1024)} KB sent, encoder: ${s.encoderImplementation ?? "unknown"}` +
        `${s.powerEfficientEncoder ? " (graphics card)" : ""}, limited by: ${s.qualityLimitationReason ?? "nothing"}`;
      if (s.framesEncoded) log.info(conn.area, msg);
      else log.error(conn.area, `${msg}. No video is going out!`);
    }
    if (s.type === "inbound-rtp") {
      const codec = stats.get(s.codecId)?.mimeType?.replace("video/", "") ?? "?";
      const msg =
        `Video ${conn.who} after 5s: ${codec}, ${s.framesDecoded ?? 0} frames decoded, ` +
        `${s.frameWidth ?? "?"}x${s.frameHeight ?? "?"} at ${s.framesPerSecond ?? 0} fps, ` +
        `${Math.round((s.bytesReceived ?? 0) / 1024)} KB received, decoder: ${s.decoderImplementation ?? "unknown"}` +
        `${s.powerEfficientDecoder ? " (graphics card)" : ""}`;
      if (s.framesDecoded) log.info(conn.area, msg);
      else if (s.bytesReceived) log.error(conn.area, `${msg}. Data arrives but no frame could be decoded!`);
      else log.error(conn.area, `${msg}. No video is coming in!`);
    }
  });
}

type HealthMemo = { at: number; bytes: Map<string, number>; frames: Map<string, number> };

/** Uma linha a cada 30s com a saúde do vídeo/áudio enquanto conectado. */
async function describeHealth(conn: Instrumented, memo: HealthMemo) {
  const stats = await getStats(conn);
  if (!stats) return;
  const now = Date.now();
  const seconds = (now - memo.at) / 1000 || 1;
  memo.at = now;
  const pair = selectedPair(stats);
  const parts: string[] = [];
  stats.forEach((s) => {
    if (s.type !== "outbound-rtp" && s.type !== "inbound-rtp") return;
    const bytes = s.bytesSent ?? s.bytesReceived ?? 0;
    const rate = ((bytes - (memo.bytes.get(s.id) ?? 0)) * 8) / seconds;
    memo.bytes.set(s.id, bytes);
    if (s.type === "outbound-rtp") {
      const remoteIn = [...stats.values()].find((r) => r.type === "remote-inbound-rtp" && r.localId === s.id);
      const lost = remoteIn?.fractionLost != null ? `${(remoteIn.fractionLost * 100).toFixed(1)}% lost` : "loss ?";
      if (s.kind === "video") {
        parts.push(
          `video out ${kbps(rate)}, ${s.frameWidth ?? "?"}x${s.frameHeight ?? "?"} at ${s.framesPerSecond ?? 0} fps, ` +
            `${lost}, ${s.nackCount ?? 0} resend requests, ${s.pliCount ?? 0} keyframe requests, ` +
            `limited by ${s.qualityLimitationReason ?? "nothing"}`,
        );
      } else parts.push(`audio out ${kbps(rate)}, ${lost}`);
    } else {
      const total = (s.packetsReceived ?? 0) + (s.packetsLost ?? 0);
      const lost = total ? `${(((s.packetsLost ?? 0) / total) * 100).toFixed(1)}% lost overall` : "loss ?";
      if (s.kind === "video") {
        const frames = s.framesDecoded ?? 0;
        const fps = (frames - (memo.frames.get(s.id) ?? 0)) / seconds;
        memo.frames.set(s.id, frames);
        const delay = s.jitterBufferEmittedCount ? ms(s.jitterBufferDelay / s.jitterBufferEmittedCount) : "?";
        parts.push(
          `video in ${kbps(rate)}, ${s.frameWidth ?? "?"}x${s.frameHeight ?? "?"} at ${fps.toFixed(0)} fps, ` +
            `${lost}, ${s.framesDropped ?? 0} frames dropped, ${s.freezeCount ?? 0} freezes ` +
            `(${(s.totalFreezesDuration ?? 0).toFixed(1)}s), buffer delay ${delay}`,
        );
      } else parts.push(`audio in ${kbps(rate)}, ${lost}, jitter ${ms(s.jitter)}`);
    }
  });
  const net = pair ? `round trip ${ms(pair.currentRoundTripTime)}, estimated upload ${kbps(pair.availableOutgoingBitrate)}` : "";
  log.info(conn.area, `Health ${conn.who} ${since(conn)}: ${[net, ...parts].filter(Boolean).join("; ") || "no media"}`);
}

// ---------- DTLS ----------

function dtlsOf(pc: RTCPeerConnection): RTCDtlsTransport | null {
  try {
    const transceiver = pc.getTransceivers?.().find((t) => t.sender.transport || t.receiver.transport);
    return transceiver?.sender.transport ?? transceiver?.receiver.transport ?? null;
  } catch {
    return null;
  }
}

/** Acompanha a criptografia (DTLS) e as trocas de rota, quando existirem. */
function watchTransport(conn: Instrumented): boolean {
  const dtls = dtlsOf(conn.pc);
  if (!dtls) return false;
  dtls.addEventListener("statechange", () => {
    const state = dtls.state;
    const msg = `Encryption handshake (DTLS) ${conn.who}: ${state} ${since(conn)}`;
    if (state === "failed") log.error(conn.area, msg);
    else log.info(conn.area, msg);
  });
  dtls.addEventListener("error", (e) => {
    const err = (e as RTCErrorEvent).error;
    log.error(
      conn.area,
      `Encryption (DTLS) error ${conn.who}: ${err?.errorDetail ?? "?"}` +
        `${err?.receivedAlert != null ? `, alert received ${err.receivedAlert}` : ""}` +
        `${err?.sentAlert != null ? `, alert sent ${err.sentAlert}` : ""}: ${err?.message ?? ""}`,
    );
  });
  const ice = dtls.iceTransport;
  ice?.addEventListener("selectedcandidatepairchange", () => {
    const pair = ice.getSelectedCandidatePair?.();
    const local = parseCandidate(pair?.local?.candidate);
    const remote = parseCandidate(pair?.remote?.candidate);
    if (!local || !remote) return;
    log.info(conn.area, `Now using route ${conn.who}: ${describeRoute(local)} -> ${describeRoute(remote)} ${since(conn)}`);
  });
  return true;
}

// ---------- ligação ----------

/** Liga os logs de estado de uma conexão; `onState` recebe cada mudança. */
export function instrument(conn: Instrumented, onState?: (state: RTCPeerConnectionState) => void) {
  const { pc, area, who } = conn;
  let candidateErrors = 0;
  let lastState = "";
  let transportWatched = false;
  let health: ReturnType<typeof setInterval> | undefined;
  const stopHealth = () => {
    clearInterval(health);
    health = undefined;
  };

  pc.onsignalingstatechange = () => {
    log.info(area, `Messages ${who}: ${pc.signalingState} ${since(conn)}`);
    if (!transportWatched) transportWatched = watchTransport(conn);
  };
  pc.onicegatheringstatechange = () => {
    if (pc.iceGatheringState === "gathering") log.info(area, `Looking for network routes ${who} ${since(conn)}`);
    if (pc.iceGatheringState === "complete") {
      const types = conn.localRoutes.map((r) => r.type);
      log.info(area, `Finished looking for routes ${who} ${since(conn)}: ${summarizeRoutes(types)}`);
      log.info(area, `Router here: ${describeNat(conn.localRoutes)}`);
      if (!types.includes("srflx")) {
        log.warn(area, `No public address found ${who}: other people outside this network may not be able to connect`);
      }
    }
  };
  pc.onicecandidateerror = (e) => {
    const ev = e as RTCPeerConnectionIceErrorEvent;
    candidateErrors++;
    if (candidateErrors <= MAX_CANDIDATE_ERRORS) {
      const meaning = STUN_ERRORS[ev.errorCode] ?? ev.errorText;
      log.warn(
        area,
        `Could not reach ${ev.url} ${who} from ${ev.address ?? "?"}:${ev.port ?? "?"} ` +
          `(code ${ev.errorCode}: ${meaning}) ${since(conn)}`,
      );
    } else if (candidateErrors === MAX_CANDIDATE_ERRORS + 1) {
      log.warn(area, `More errors reaching the STUN servers ${who}, not listing them`);
    }
  };
  pc.oniceconnectionstatechange = () => {
    const state = pc.iceConnectionState;
    const msg = `Route checks (ICE) ${who}: ${state} ${since(conn)}`;
    if (state === "failed") log.error(area, msg);
    else if (state === "disconnected") log.warn(area, msg);
    else log.info(area, msg);
  };
  pc.onconnectionstatechange = () => {
    const state = pc.connectionState;
    onState?.(state);
    if (state === lastState || state === "new") return;
    lastState = state;
    if (!transportWatched) transportWatched = watchTransport(conn);
    switch (state) {
      case "connecting":
        log.info(area, `Connecting ${who}... ${since(conn)}`);
        break;
      case "connected": {
        describeConnected(conn).catch(() => {});
        setTimeout(() => describeVideo(conn).catch(() => {}), 5000);
        stopHealth();
        const memo: HealthMemo = { at: Date.now(), bytes: new Map(), frames: new Map() };
        health = setInterval(() => {
          if (pc.connectionState !== "connected") return stopHealth();
          describeHealth(conn, memo).catch(() => {});
        }, HEALTH_EVERY_MS);
        break;
      }
      case "disconnected":
        stopHealth();
        log.warn(area, `Connection ${who} dropped, trying to recover ${since(conn)}`);
        logConnectionReport(conn, "Connection dropped", "warn").catch(() => {});
        break;
      case "failed":
        stopHealth();
        logConnectionReport(conn, "Connection FAILED").catch(() => {});
        break;
      case "closed":
        stopHealth();
        break;
    }
  };
}

/** Fechei a conexão (parou de assistir, saiu, refez): registra quanto durou. */
export function noteClosed(conn: Instrumented, why: string) {
  log.info(
    conn.area,
    `Closed the connection ${conn.who} ${since(conn)} (${why}; it was "${conn.pc.connectionState}")`,
  );
}

/** Descreve as faixas que vão ser enviadas (para achar faixa morta ou vazia). */
export function describeTracks(stream: MediaStream): string {
  return stream
    .getTracks()
    .map((t) => {
      const s = t.getSettings();
      const size = t.kind === "video" && s.width ? ` ${s.width}x${s.height}` : "";
      const fps = t.kind === "video" && s.frameRate ? ` at ${Math.round(s.frameRate)} fps` : "";
      const label = t.label ? ` "${t.label}"` : "";
      return `${t.kind}${label}${size}${fps} (${t.readyState}${t.enabled ? "" : ", disabled"}${t.muted ? ", muted" : ""})`;
    })
    .join(", ");
}
