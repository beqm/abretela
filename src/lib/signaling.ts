// Conexão com o servidor da sala (WebSocket): entrar com nome e senha, tentar
// de novo enquanto o túnel ainda não responde e trocar mensagens. Não sabe
// nada de vídeo; quem decide o que fazer com cada mensagem é o `room.ts`.

import { describeError, log } from "./log";
import type { RoomSettings } from "./quality";
import type { ThumbSignal } from "./thumbs";
import type { TransportSignal } from "./transport/types";

export type PeerInfo = { id: string; name: string; sharing: boolean; label?: string | null };

/** O que uma pessoa manda para outra pela sala. */
export type SignalData = TransportSignal | ThumbSignal;

export type ServerMsg =
  | { type: "welcome"; id: string; peers: PeerInfo[]; settings: RoomSettings }
  | { type: "peer-joined"; peer: PeerInfo }
  | { type: "peer-left"; id: string }
  | { type: "peer-updated"; id: string; sharing: boolean; label?: string | null }
  | { type: "signal"; from: string; data: SignalData };

type ClientMsg =
  | { type: "join"; name: string; password: string }
  | { type: "sharing"; sharing: boolean; label?: string }
  | { type: "signal"; to: string; data: SignalData };

export class WrongPasswordError extends Error {
  constructor() {
    super("Senha incorreta.");
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export class Signaling {
  /** Cada mensagem do servidor (o "welcome" chega antes de `connect` terminar). */
  onMessage: (msg: ServerMsg) => void = () => {};
  /** A conexão caiu depois de já ter entrado na sala. */
  onClosed: () => void = () => {};

  private ws: WebSocket | null = null;
  private joined = false;
  private disposed = false;

  constructor(
    private url: string,
    private name: string,
    private password: string,
  ) {}

  /** Tenta conectar por até ~45s (o DNS de túneis novos pode demorar). */
  async connect(onAttempt?: (n: number) => void): Promise<void> {
    const deadline = Date.now() + 45_000;
    const where = this.url.startsWith("ws://127.0.0.1") ? "my own room" : this.url;
    log.info("room", `Joining ${where} as "${this.name}"`);
    for (let attempt = 1; Date.now() < deadline; attempt++) {
      if (this.disposed) throw new Error("cancelado");
      onAttempt?.(attempt);
      try {
        await this.open();
        return;
      } catch (e) {
        if (e instanceof WrongPasswordError) {
          log.warn("room", "Wrong room password");
          throw e;
        }
        if (attempt === 1 || attempt % 5 === 0) {
          log.info("room", `Could not reach the room yet (attempt ${attempt}), trying again`);
        }
        await sleep(2000);
      }
    }
    log.error("room", "Gave up joining the room after 45s");
    throw new Error("connect-failed");
  }

  private open(): Promise<void> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(this.url);
      let wrongPassword = false;
      ws.onopen = () => {
        this.ws = ws;
        this.send({ type: "join", name: this.name, password: this.password });
      };
      ws.onmessage = (ev) => {
        let msg: ServerMsg | { type: "error"; reason: string };
        try {
          msg = JSON.parse(ev.data);
        } catch {
          log.warn("room", "Got an unreadable message from the room server");
          return;
        }
        if (msg.type === "error") {
          wrongPassword = msg.reason === "wrong-password";
          return;
        }
        if (msg.type === "welcome") this.joined = true;
        try {
          this.onMessage(msg);
        } catch (e) {
          log.error("room", `Something broke while handling "${msg.type}": ${describeError(e)}`);
        }
        if (msg.type === "welcome") resolve();
      };
      ws.onclose = () => {
        if (!this.joined) reject(wrongPassword ? new WrongPasswordError() : new Error("fechado"));
        else this.onClosed();
      };
    });
  }

  send(msg: ClientMsg) {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(msg));
  }

  signal(to: string, data: SignalData) {
    this.send({ type: "signal", to, data });
  }

  /** Sai de vez: fecha a conexão e cancela as tentativas em andamento. */
  close() {
    this.disposed = true;
    this.ws?.close();
  }
}
