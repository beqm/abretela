// De onde a página de assistir tira a sala: do próprio endereço da página (o
// servidor que a serviu é a sala), com a senha depois do "#" no link do túnel.
// `?room=<url>` aponta para outra sala (testes e desenvolvimento).

export type PageRoom = {
  /** URL https da sala, sem barra no fim. */
  room: string;
  /** WebSocket da sala. */
  ws: string;
  /** Senha do link (túnel); o servidor próprio nunca manda. */
  password: string | null;
  /** Nome curto para mostrar enquanto não sabe o nome da sala. */
  label: string;
};

type Loc = { protocol: string; host: string; hash: string; search: string };

export function roomFromPage(loc: Loc): PageRoom {
  let base = `${loc.protocol}//${loc.host}`;
  const override = new URLSearchParams(loc.search).get("room");
  if (override && /^https?:\/\/[^\s/]+\/?$/i.test(override)) base = override.replace(/\/+$/, "");
  let password: string | null = null;
  if (loc.hash.length > 1) {
    try {
      password = decodeURIComponent(loc.hash.slice(1)).trim() || null;
    } catch {
      password = loc.hash.slice(1).trim() || null;
    }
  }
  const host = new URL(base).hostname;
  return {
    room: base,
    ws: `${base.replace(/^http/i, "ws")}/ws`,
    password,
    label: host.split(".")[0] || host,
  };
}

/** Link que abre a mesma sala no app (o protocolo abretela://). */
export const appLink = (p: PageRoom) =>
  `abretela://join/${p.room.replace(/^https?:\/\//i, "")}${p.password ? `#${encodeURIComponent(p.password)}` : ""}`;

export const DOWNLOAD_URL = "https://github.com/beqm/abretela/releases/latest";

/** No celular não existe o app (só Windows, por enquanto). */
export const isMobile = (ua: string) => /Android|iPhone|iPad|iPod|Mobile/i.test(ua);

const HAS_APP = "abretela.web.hasApp";

/** Este navegador já abriu o app antes (então tenta abrir sozinho). */
export function rememberedApp(storage: Pick<Storage, "getItem"> = localStorage): boolean {
  try {
    return storage.getItem(HAS_APP) === "1";
  } catch {
    return false;
  }
}

type Env = {
  win: Pick<Window, "addEventListener" | "removeEventListener">;
  doc: Pick<Document, "addEventListener" | "removeEventListener" | "hidden">;
  storage: Pick<Storage, "setItem" | "removeItem">;
  go: (url: string) => void;
};

/**
 * Tenta abrir a sala no app. O navegador não deixa saber se um programa está
 * instalado, então a pista é a aba perder o foco logo depois (o app abriu na
 * frente). `onSlow`: em 2,5 s nada aconteceu (talvez não tenha o app, ou o
 * navegador está perguntando se pode abrir); continua de olho por 30 s.
 * Devolve uma função que para de olhar.
 */
export function tryOpenApp(
  link: string,
  on: { opened: () => void; slow: () => void },
  env: Env = {
    win: window,
    doc: document,
    storage: localStorage,
    go: (url) => (location.href = url),
  },
): () => void {
  let done = false;
  const stop = () => {
    done = true;
    env.win.removeEventListener("blur", left);
    env.doc.removeEventListener("visibilitychange", left);
    clearTimeout(slow);
    clearTimeout(giveUp);
  };
  function left() {
    if (done) return;
    stop();
    try {
      env.storage.setItem(HAS_APP, "1");
    } catch {}
    on.opened();
  }
  env.win.addEventListener("blur", left);
  env.doc.addEventListener("visibilitychange", left);
  const slow = setTimeout(() => !done && on.slow(), 2500);
  const giveUp = setTimeout(() => {
    if (done) return;
    stop();
    // não abriu: não tenta mais sozinho nas próximas vezes
    try {
      env.storage.removeItem(HAS_APP);
    } catch {}
  }, 30_000);
  env.go(link);
  return stop;
}
