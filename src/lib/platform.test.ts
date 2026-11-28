import { afterEach, describe, expect, it, vi } from "vitest";
import { isLinux, isMac, windowFollowsElementFullscreen } from "./platform";

afterEach(() => vi.unstubAllGlobals());

describe("em que sistema o app está", () => {
  it("reconhece o WKWebView do macOS", () => {
    vi.stubGlobal("navigator", {
      userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)",
    });
    expect(isMac()).toBe(true);
  });

  it("no macOS a tela cheia de um elemento já ocupa a tela: a janela fica como está", () => {
    vi.stubGlobal("navigator", {
      userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)",
    });
    expect(windowFollowsElementFullscreen()).toBe(false);
  });

  it("no Windows a tela cheia de um elemento só enche a janela, que vai junto", () => {
    vi.stubGlobal("navigator", {
      userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0",
    });
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {} });
    expect(windowFollowsElementFullscreen()).toBe(true);
  });

  it("no navegador (página de assistir) não há janela do app para pôr em tela cheia", () => {
    vi.stubGlobal("navigator", {
      userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    });
    vi.stubGlobal("window", {});
    expect(windowFollowsElementFullscreen()).toBe(false);
  });

  it("não confunde com o WebView2 do Windows", () => {
    vi.stubGlobal("navigator", {
      userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0",
    });
    expect(isMac()).toBe(false);
  });
});

describe("o Linux", () => {
  const ua = (userAgent: string) => vi.stubGlobal("navigator", { userAgent });

  it("reconhece o WebKitGTK", () => {
    ua("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15");
    expect(isLinux()).toBe(true);
    expect(isMac()).toBe(false);
  });

  it("não confunde com o Android, que também se diz Linux", () => {
    ua("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36");
    expect(isLinux()).toBe(false);
  });

  it("nem com o Windows ou o Mac", () => {
    ua("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36");
    expect(isLinux()).toBe(false);
    ua("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)");
    expect(isLinux()).toBe(false);
  });
});
