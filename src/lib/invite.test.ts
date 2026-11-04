import { describe, expect, it } from "vitest";
import { makeInvite, parseInvite, roomLabel } from "./room";

const ROOM = "https://abc-def.trycloudflare.com";
const WS = "wss://abc-def.trycloudflare.com/ws";

describe("parseInvite", () => {
  it("lê o link de convite com senha", () => {
    expect(parseInvite(`${ROOM}/#123456`)).toEqual({ room: ROOM, ws: WS, password: "123456" });
  });

  it("lê o link que chega pelo protocolo abretela://", () => {
    expect(parseInvite("abretela://join/abc-def.trycloudflare.com#99")).toEqual({ room: ROOM, ws: WS, password: "99" });
  });

  it("aceita os convites antigos (só o código do túnel)", () => {
    expect(parseInvite("Abc-Def#55")).toEqual({ room: ROOM, ws: WS, password: "55" });
  });

  it("aceita só o endereço, sem senha", () => {
    expect(parseInvite("abc-def.trycloudflare.com")).toEqual({ room: ROOM, ws: WS, password: null });
  });

  it("ignora espaços em volta", () => {
    expect(parseInvite(`  ${ROOM}/#  12 `)?.password).toBe("12");
  });

  it("aceita o endereço do WebSocket local", () => {
    expect(parseInvite("ws://localhost:1234/ws")).toEqual({
      room: "http://localhost:1234",
      ws: "ws://localhost:1234/ws",
      password: null,
    });
  });

  it("mantém o caminho, sem a barra no fim", () => {
    expect(parseInvite("https://example.com/sala/")?.room).toBe("https://example.com/sala");
  });

  it("recusa o que não é convite", () => {
    for (const bad of ["", "   ", "#123", "ftp://example.com", "hello world"]) {
      expect(parseInvite(bad), bad).toBeNull();
    }
  });
});

describe("makeInvite", () => {
  it("põe a senha depois do #", () => {
    expect(makeInvite(ROOM, "123456")).toBe(`${ROOM}/#123456`);
  });

  it("volta igual ao ler, mesmo com caracteres especiais", () => {
    const password = "a b#ç/?";
    expect(parseInvite(makeInvite(ROOM, password))).toEqual({ room: ROOM, ws: WS, password });
  });
});

describe("roomLabel", () => {
  it("mostra o primeiro pedaço do endereço", () => {
    expect(roomLabel("https://ancient-kevin-hollow-tribe.trycloudflare.com")).toBe("ancient-kevin-hollow-tribe");
  });

  it("devolve o texto como veio se não for URL", () => {
    expect(roomLabel("não é url")).toBe("não é url");
  });
});
