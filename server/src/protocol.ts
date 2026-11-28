// Tipos e limites do servidor da sala. As mensagens são as mesmas do servidor
// do túnel (`src-tauri/src/room/server.rs`): `join`, `welcome`, `peer-*`,
// `signal`, `ice-servers`. O que é novo (dono, kikar, configurações) as versões
// antigas do app ignoram.

/**
 * Muda quando o código do servidor muda: o app compara com o do Worker publicado
 * e oferece "Atualizar servidor". 2: tira da sala as conexões sem ping. 3: página de assistir pelo navegador. 4: página nova (abrir no app primeiro).
 */
export const SERVER_VERSION = 4;

/** Limites de qualidade da sala (mesmos nomes do app e do servidor do túnel). */
export type RoomSettings = { minHeight: number; maxHeight: number; minFps: number; maxFps: number };

export const DEFAULT_SETTINGS: RoomSettings = { minHeight: 360, maxHeight: 1080, minFps: 15, maxFps: 60 };

/** O que o dono configura; guardado no SQLite da sala. */
export type RoomConfig = {
  name: string;
  /** `null`: o dono ainda não escolheu a senha (só ele entra). */
  password: PasswordHash | null;
  settings: RoomSettings;
  /** Quanto dura a senha do TURN de cada pessoa, em minutos. */
  turnLifetime: number;
};

export type PasswordHash = { salt: string; hash: string; iterations: number };

export const DEFAULT_CONFIG: RoomConfig = {
  name: "AbreTela",
  password: null,
  settings: DEFAULT_SETTINGS,
  turnLifetime: 60,
};

/** Os mesmos tempos que o app oferece para o TURN do anfitrião. */
export const TURN_LIFETIMES = [15, 30, 60, 120, 360, 1440];

/** Uma pessoa como os outros veem (o `PeerInfo` do app, mais `owner` e `web`). */
export type PeerInfo = { id: string; name: string; sharing: boolean; label: string | null; owner: boolean; web: boolean };

/** Uma pessoa na sala, guardada no próprio WebSocket (sobrevive à hibernação). */
export type Peer = PeerInfo & {
  joinedAt: number;
  /** Credencial de TURN dela, se o servidor tiver TURN. */
  turn?: { username: string; expiresAt: number; retryAt?: number };
};

/** O que fica preso a cada WebSocket. `peer` só existe depois do `join`. */
export type ConnState = { ip: string; peer?: Peer };

export type Ban = { id: string; name: string; ipHash: string; until: number };

export const LIMITS = {
  /** Pessoas na sala ao mesmo tempo. */
  maxPeers: 50,
  /** Tamanho máximo de uma mensagem (as prévias das telas vão por aqui). */
  maxMessageBytes: 1024 * 1024,
  /** Senhas erradas por endereço antes de esperar. */
  maxWrongPasswords: 5,
  wrongPasswordWindowMs: 60_000,
  /** Atraso de cada senha errada (dificulta tentativa e erro). */
  wrongPasswordDelayMs: 1000,
  nameLength: 32,
  labelLength: 48,
  roomNameLength: 40,
  passwordLength: 64,
} as const;

/** Por quanto tempo a credencial de TURN antiga vale depois da nova chegar. */
export const TURN_REVOKE_GRACE_MS = 2 * 60_000;
/** Se a Cloudflare falhar ao renovar, tenta de novo depois disto. */
export const TURN_RETRY_MS = 60_000;

/** Quando renovar: uns minutos antes de vencer (igual ao app). */
export function renewAt(expiresAt: number, lifetimeMinutes: number): number {
  const marginMinutes = Math.min(5, lifetimeMinutes / 4);
  return expiresAt - marginMinutes * 60_000;
}

/**
 * O app manda `ping` a cada 30 s (a Cloudflare responde sem acordar a sala e
 * anota a hora). Quem fica mais que isto sem ping caiu sem avisar (janela
 * recarregada, internet que sumiu) e sai da sala.
 */
export const STALE_MS = 75_000;
/** Com gente na sala, de quanto em quanto tempo procura conexões mortas. */
export const SWEEP_MS = 60_000;

/** Códigos de fechamento do WebSocket. */
export const CLOSE = {
  timeout: 4000,
  kicked: 4001,
  refused: 4003,
} as const;
