import { afterEach, describe, expect, it, vi } from "vitest";
import {
  canReceiveInWebView,
  canSendFromWebView,
  isShareCancelled,
  shouldSendNatively,
  systemPickedSource,
} from "./support";

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

describe("quem recebe a tela dos outros", () => {
  it("com RTCPeerConnection (WebView2, WKWebView), o próprio WebView", () => {
    vi.stubGlobal("RTCPeerConnection", class {});
    expect(canReceiveInWebView()).toBe(true);
  });

  it("sem ele (WebKitGTK do Linux), o app recebe", () => {
    vi.stubGlobal("RTCPeerConnection", undefined);
    expect(canReceiveInWebView()).toBe(false);
  });
});

describe("quem escolhe a tela", () => {
  const source = (kind: string, id = kind) => ({ id, kind });

  it("com telas e janelas na lista (Windows, macOS), a pessoa escolhe no app", () => {
    expect(systemPickedSource([source("screen"), source("window")])).toBeNull();
    expect(systemPickedSource([source("screen")])).toBeNull();
    expect(systemPickedSource([])).toBeNull();
  });

  it("com a fonte do sistema sozinha (Linux), é o sistema que pergunta", () => {
    expect(systemPickedSource([source("system", "portal")])).toEqual({ id: "portal", kind: "system" });
  });

  it("fechar a janela do sistema sem escolher não é erro", () => {
    expect(isShareCancelled("cancelled")).toBe(true);
    expect(isShareCancelled(new Error("cancelled"))).toBe(false);
    expect(isShareCancelled("the system's screen sharing portal failed: timeout")).toBe(false);
    expect(isShareCancelled(undefined)).toBe(false);
  });
});
