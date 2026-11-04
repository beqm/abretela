// Convite para uma sala: o link que se compartilha e como lê-lo de volta.

/**
 * Link de convite: a URL pública da sala (que abre a página do convite) com a
 * senha depois do "#" (o navegador nunca envia essa parte ao servidor).
 */
export const makeInvite = (roomUrl: string, password: string) => `${roomUrl}/#${encodeURIComponent(password)}`;

export type Invite = {
  /** URL https pública da sala, sem barra no fim. */
  room: string;
  /** Endereço do WebSocket de sinalização. */
  ws: string;
  password: string | null;
};

/** Nome curto para mostrar a sala (o primeiro pedaço do endereço). */
export const roomLabel = (roomUrl: string) => {
  try {
    return new URL(roomUrl).hostname.split(".")[0];
  } catch {
    return roomUrl;
  }
};

/**
 * Lê um convite. Aceita o link (https://endereço/#senha), o mesmo link vindo
 * pelo protocolo (abretela://join/endereço#senha), só o endereço (com ou sem
 * "#senha") e, por compatibilidade, os convites antigos que eram só o código
 * do túnel do trycloudflare ("abc-def#senha").
 */
export function parseInvite(input: string): Invite | null {
  const raw = input.trim().replace(/^abretela:\/\/join\//i, "https://");
  const hashAt = raw.indexOf("#");
  let password = hashAt >= 0 ? raw.slice(hashAt + 1).trim() : "";
  try {
    password = decodeURIComponent(password);
  } catch {}
  let base = (hashAt >= 0 ? raw.slice(0, hashAt) : raw).trim();
  if (!base) return null;

  if (/^[a-z0-9-]+$/i.test(base)) {
    base = `https://${base.toLowerCase()}.trycloudflare.com`; // convite antigo
  } else if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(base)) {
    base = `https://${base}`;
  }
  let url: URL;
  try {
    url = new URL(base.replace(/^ws(s?):/i, "http$1:"));
  } catch {
    return null;
  }
  if (!/^https?:$/.test(url.protocol) || (!url.hostname.includes(".") && url.hostname !== "localhost")) return null;
  const path = url.pathname.replace(/\/+$/, "").replace(/\/ws$/, "");
  const room = `${url.protocol}//${url.host}${path}`;
  return { room, ws: `${room.replace(/^http/, "ws")}/ws`, password: password || null };
}
