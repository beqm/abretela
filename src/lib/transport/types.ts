// Contrato do transporte de mídia: como minha tela chega a quem assiste e
// como a tela dos outros chega até mim. Hoje só existe a malha (`mesh.ts`, uma
// conexão por par); um SFU ou um repasse seriam outras implementações. O
// resto do app (sala, interface) só conhece esta interface.

import type { Quality } from "../quality";

/** Mensagens que o transporte troca com os outros pela sinalização da sala. */
export type TransportSignal =
  | { kind: "watch" }
  | { kind: "unwatch" }
  | { kind: "offer"; sdp: string }
  | { kind: "answer"; sdp: string }
  | { kind: "ice"; role: "sharer" | "viewer"; candidate: RTCIceCandidateInit }
  /** Tamanho em que quem assiste está vendo a tela (altura em pixels; 0 = app escondido). */
  | { kind: "view"; height: number };

/** A tela de alguém, como eu recebo. */
export type RemoteView = {
  stream: MediaStream | null;
  state: "connecting" | "connected" | "failed";
};

/** O que eu estou transmitindo. */
export type Publication = {
  stream: MediaStream;
  quality: Quality;
  /** Prefere H.264 (codificado pela placa de vídeo) ao VP8 (no processador). */
  hwEncode: boolean;
  /** Altura real do último frame capturado (0 se ainda não se sabe). */
  sourceHeight: () => number;
};

/** O que o transporte precisa da sala. */
export type TransportHost = {
  /** Envia uma mensagem a outra pessoa pela sinalização. */
  signal: (to: string, data: TransportSignal) => void;
  /** Nome para os logs (ex.: '"Ana"'). */
  nameOf: (id: string) => string;
  /** A tela de alguém que eu assisto mudou (chegou o vídeo, conectou, falhou). */
  onRemote: (id: string, patch: Partial<RemoteView>) => void;
};

export interface MediaTransport {
  // ---------- quem transmite ----------

  /** Começa a transmitir (quem quiser assistir pede depois). */
  publish(pub: Publication): void;
  /**
   * Troca o que estou transmitindo sem derrubar quem assiste. Devolve `false`
   * se as faixas mudaram (ex.: sem áudio agora) e foi preciso reconectar.
   */
  replace(pub: Publication): boolean;
  /** Para de transmitir e fecha o envio para todos. */
  unpublish(): void;

  // ---------- quem assiste ----------

  /** Pede para assistir alguém; o vídeo chega por `TransportHost.onRemote`. */
  watch(peerId: string): void;
  /** Para de assistir; `tell` avisa o outro lado (não precisa se ele saiu). */
  stopWatching(peerId: string, tell: boolean): void;
  /** Avisa a altura em que estou vendo alguém (0 = app escondido), só quando muda. */
  requestView(peerId: string, height: number): void;
  /** Em que etapa está a conexão com alguém (para o log de "travou"). */
  describeStuck(peerId: string): string;

  // ---------- geral ----------

  /** Mensagem do transporte que chegou de outra pessoa. */
  handleSignal(from: string, data: TransportSignal): void;
  /** A pessoa saiu da sala: esquece tudo dela. */
  removePeer(peerId: string): void;
  /** Fecha tudo (saí da sala). */
  close(): void;
}
