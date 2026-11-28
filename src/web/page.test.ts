import { afterEach, describe, expect, it, vi } from "vitest";
import { appLink, isMobile, rememberedApp, roomFromPage, tryOpenApp } from "./page";

const loc = (url: string) => {
  const u = new URL(url);
  return { protocol: u.protocol, host: u.host, hash: u.hash, search: u.search };
};

describe("roomFromPage", () => {
  it("túnel: a sala é o próprio endereço, e a senha vem depois do #", () => {
    expect(roomFromPage(loc("https://abc-def-ghi.trycloudflare.com/watch#123%20456"))).toEqual({
      room: "https://abc-def-ghi.trycloudflare.com",
      ws: "wss://abc-def-ghi.trycloudflare.com/ws",
      password: "123 456",
      label: "abc-def-ghi",
    });
  });

  it("servidor próprio: sem senha no link", () => {
    const p = roomFromPage(loc("https://abretela-1a2b3c.breno.workers.dev/"));
    expect(p.password).toBeNull();
    expect(p.ws).toBe("wss://abretela-1a2b3c.breno.workers.dev/ws");
  });

  it("?room= aponta para outra sala (desenvolvimento), só se for um endereço http(s)", () => {
    expect(roomFromPage(loc("http://localhost:1430/watch.html?room=http://127.0.0.1:8787")).ws).toBe("ws://127.0.0.1:8787/ws");
    expect(roomFromPage(loc("http://localhost:1430/watch.html?room=javascript:alert(1)")).ws).toBe("ws://localhost:1430/ws");
  });

  it("link para abrir no app leva o endereço e a senha", () => {
    expect(appLink(roomFromPage(loc("https://x.trycloudflare.com/watch#4321")))).toBe("abretela://join/x.trycloudflare.com#4321");
    expect(appLink(roomFromPage(loc("https://s.workers.dev/")))).toBe("abretela://join/s.workers.dev");
  });
});

describe("abrir no app", () => {
  afterEach(() => vi.useRealTimers());

  /** Janela, documento e armazenamento de mentira. */
  function env() {
    const handlers: Record<string, () => void> = {};
    const store = new Map<string, string>();
    const target = {
      addEventListener: (ev: string, fn: () => void) => void (handlers[ev] = fn),
      removeEventListener: (ev: string) => void delete handlers[ev],
    };
    const opened: string[] = [];
    return {
      handlers,
      store,
      opened,
      env: {
        win: target,
        doc: { ...target, hidden: false },
        storage: { setItem: (k: string, v: string) => void store.set(k, v), removeItem: (k: string) => void store.delete(k) },
        go: (url: string) => void opened.push(url),
      },
    };
  }

  it("a aba perdeu o foco: o app abriu, e fica lembrado", () => {
    vi.useFakeTimers();
    const e = env();
    const on = { opened: vi.fn(), slow: vi.fn() };
    tryOpenApp("abretela://join/x", on, e.env as never);
    expect(e.opened).toEqual(["abretela://join/x"]);
    e.handlers.blur();
    expect(on.opened).toHaveBeenCalledOnce();
    expect(rememberedApp({ getItem: (k) => e.store.get(k) ?? null })).toBe(true);
    vi.advanceTimersByTime(30_000);
    expect(on.slow).not.toHaveBeenCalled();
  });

  it("nada aconteceu: avisa em 2,5 s e, depois de 30 s, esquece", () => {
    vi.useFakeTimers();
    const e = env();
    e.store.set("abretela.web.hasApp", "1");
    const on = { opened: vi.fn(), slow: vi.fn() };
    tryOpenApp("abretela://join/x", on, e.env as never);
    vi.advanceTimersByTime(2500);
    expect(on.slow).toHaveBeenCalledOnce();
    vi.advanceTimersByTime(30_000);
    expect(e.store.has("abretela.web.hasApp")).toBe(false);
    expect(on.opened).not.toHaveBeenCalled();
  });

  it("abriu depois de o navegador perguntar (depois do aviso): ainda conta", () => {
    vi.useFakeTimers();
    const e = env();
    const on = { opened: vi.fn(), slow: vi.fn() };
    tryOpenApp("abretela://join/x", on, e.env as never);
    vi.advanceTimersByTime(8000);
    e.handlers.visibilitychange();
    expect(on.opened).toHaveBeenCalledOnce();
  });

  it("celular não tem o app", () => {
    expect(isMobile("Mozilla/5.0 (Linux; Android 14) Mobile Safari")).toBe(true);
    expect(isMobile("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/140")).toBe(false);
  });
});
