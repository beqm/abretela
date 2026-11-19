import { afterEach, describe, expect, it, vi } from "vitest";
import { canSendFromWebView, shouldSendNatively } from "./support";

afterEach(() => vi.unstubAllGlobals());

describe("quem manda a minha tela", () => {
  it("com MediaStreamTrackGenerator (WebView2), o WebView consegue e a opção decide", () => {
    vi.stubGlobal("MediaStreamTrackGenerator", class {});
    expect(canSendFromWebView()).toBe(true);
    expect(shouldSendNatively(false)).toBe(false);
    expect(shouldSendNatively(true)).toBe(true);
  });

  it("sem ele (WKWebView do macOS), o envio nativo é obrigatório", () => {
    vi.stubGlobal("MediaStreamTrackGenerator", undefined);
    expect(canSendFromWebView()).toBe(false);
    expect(shouldSendNatively(false)).toBe(true);
    expect(shouldSendNatively(true)).toBe(true);
  });
});
