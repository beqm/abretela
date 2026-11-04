// Logs das conexões WebRTC: por qual rota conectou, se o vídeo está saindo ou
// chegando e, quando falha, quais rotas cada lado achou.

import { log } from "../log";

/** Uma conexão com alguém, com o que precisa para os logs. */
export type Instrumented = {
  pc: RTCPeerConnection;
  /** "sharing" (eu transmito) ou "watching" (eu assisto). */
  area: "sharing" | "watching";
  /** Ex.: 'to "Ana"' ou 'from "Ana"'. */
  who: string;
  /** Tipos de rota (host/srflx/relay) achados aqui e recebidos do outro lado. */
  localRoutes: string[];
  remoteRoutes: string[];
};

const ROUTE_NAMES: Record<string, string> = {
  host: "local network",
  srflx: "public address (STUN)",
  prflx: "public address (discovered)",
  relay: "relay (TURN)",
};

/** Ex.: "2 local network, 1 public address (STUN)". */
export function summarizeRoutes(types: string[]): string {
  if (!types.length) return "none";
  const counts = new Map<string, number>();
  types.forEach((t) => counts.set(t, (counts.get(t) ?? 0) + 1));
  return [...counts].map(([t, n]) => `${n} ${ROUTE_NAMES[t] ?? t}`).join(", ");
}

/** Lê do getStats qual rota de rede foi escolhida. */
async function describeConnected(conn: Instrumented) {
  const stats = await conn.pc.getStats();
  let route = "unknown route";
  stats.forEach((s) => {
    if (s.type === "candidate-pair" && s.nominated && s.state === "succeeded") {
      const local = stats.get(s.localCandidateId);
      const remote = stats.get(s.remoteCandidateId);
      route =
        `${ROUTE_NAMES[local?.candidateType] ?? local?.candidateType} -> ` +
        `${ROUTE_NAMES[remote?.candidateType] ?? remote?.candidateType} over ${local?.protocol ?? "?"}`;
    }
  });
  log.info(conn.area, `Connected ${conn.who} via ${route}`);
}

/** Alguns segundos depois de conectar: o vídeo está mesmo saindo/chegando? */
async function describeVideo(conn: Instrumented) {
  if (conn.pc.connectionState !== "connected") return;
  const stats = await conn.pc.getStats();
  stats.forEach((s) => {
    if (s.kind !== "video") return;
    if (s.type === "outbound-rtp") {
      const codec = stats.get(s.codecId)?.mimeType?.replace("video/", "") ?? "?";
      const msg =
        `Video ${conn.who} after 5s: ${codec}, ${s.framesEncoded ?? 0} frames encoded, ` +
        `${s.frameWidth ?? "?"}x${s.frameHeight ?? "?"} at ${s.framesPerSecond ?? 0} fps, ` +
        `${Math.round((s.bytesSent ?? 0) / 1024)} KB sent, encoder: ${s.encoderImplementation ?? "unknown"}, ` +
        `limited by: ${s.qualityLimitationReason ?? "nothing"}`;
      if (s.framesEncoded) log.info(conn.area, msg);
      else log.error(conn.area, `${msg}. No video is going out!`);
    }
    if (s.type === "inbound-rtp") {
      const msg =
        `Video ${conn.who} after 5s: ${s.framesDecoded ?? 0} frames decoded, ` +
        `${s.frameWidth ?? "?"}x${s.frameHeight ?? "?"} at ${s.framesPerSecond ?? 0} fps, ` +
        `${Math.round((s.bytesReceived ?? 0) / 1024)} KB received, decoder: ${s.decoderImplementation ?? "unknown"}`;
      if (s.framesDecoded) log.info(conn.area, msg);
      else log.error(conn.area, `${msg}. No video is coming in!`);
    }
  });
}

/** Liga os logs de estado de uma conexão; `onState` recebe cada mudança. */
export function instrument(conn: Instrumented, onState?: (state: RTCPeerConnectionState) => void) {
  const { pc } = conn;
  let candidateErrors = 0;
  let lastState = "";
  pc.onicegatheringstatechange = () => {
    if (pc.iceGatheringState === "complete") {
      log.info(conn.area, `Network routes found on this PC ${conn.who}: ${summarizeRoutes(conn.localRoutes)}`);
    }
  };
  pc.onicecandidateerror = (e) => {
    const ev = e as RTCPeerConnectionIceErrorEvent;
    if (candidateErrors++ < 3) {
      log.warn(conn.area, `Could not reach ${ev.url} ${conn.who} (code ${ev.errorCode}: ${ev.errorText})`);
    }
  };
  pc.onconnectionstatechange = () => {
    const state = pc.connectionState;
    onState?.(state);
    if (state === lastState || state === "new") return;
    lastState = state;
    switch (state) {
      case "connecting":
        log.info(conn.area, `Connecting ${conn.who}...`);
        break;
      case "connected":
        describeConnected(conn).catch(() => {});
        setTimeout(() => describeVideo(conn).catch(() => {}), 5000);
        break;
      case "disconnected":
        log.warn(conn.area, `Connection ${conn.who} dropped, trying to recover`);
        break;
      case "failed":
        log.error(
          conn.area,
          `Connection ${conn.who} FAILED. Routes on this PC: ${summarizeRoutes(conn.localRoutes)}. ` +
            `Routes from the other PC: ${summarizeRoutes(conn.remoteRoutes)}`,
        );
        break;
    }
  };
}

/** Descreve as faixas que vão ser enviadas (para achar faixa morta ou vazia). */
export function describeTracks(stream: MediaStream): string {
  return stream
    .getTracks()
    .map((t) => {
      const s = t.getSettings();
      const size = t.kind === "video" && s.width ? ` ${s.width}x${s.height}` : "";
      return `${t.kind}${size} (${t.readyState}${t.enabled ? "" : ", disabled"}${t.muted ? ", muted" : ""})`;
    })
    .join(", ");
}
