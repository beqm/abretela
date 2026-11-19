import { afterEach, describe, expect, it, vi } from "vitest";
import { isMac } from "./platform";

afterEach(() => vi.unstubAllGlobals());

describe("em que sistema o app está", () => {
  it("reconhece o WKWebView do macOS", () => {
    vi.stubGlobal("navigator", {
      userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)",
    });
    expect(isMac()).toBe(true);
  });

  it("não confunde com o WebView2 do Windows", () => {
    vi.stubGlobal("navigator", {
      userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0",
    });
    expect(isMac()).toBe(false);
  });
});
