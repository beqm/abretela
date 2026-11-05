// Envia linhas de log para o arquivo diário (escrito pelo Rust). Os textos
// são em inglês e escritos para gente ler, não para máquina.

import { invoke } from "@tauri-apps/api/core";

type Level = "info" | "warn" | "error";

function send(level: Level, area: string, message: string) {
  invoke("log", { level, area, message }).catch(() => {});
}

export const log = {
  info: (area: string, message: string) => send("info", area, message),
  warn: (area: string, message: string) => send("warn", area, message),
  error: (area: string, message: string) => send("error", area, message),
};

/** Escreve no log as placas de rede do PC (IPs, VPNs, gateway); `why` diz quando. */
export function logNetwork(why: string) {
  invoke("log_network", { why }).catch(() => {});
}

/** Texto legível de um erro qualquer (inclui o nome, ex.: "InvalidAccessError: ..."). */
export function describeError(e: unknown): string {
  if (e instanceof Error) return e.name && e.name !== "Error" ? `${e.name}: ${e.message}` : e.message;
  if (typeof e === "string") return e;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

/**
 * Registra no arquivo os erros que escapariam: exceções não tratadas,
 * promessas rejeitadas e `console.error`/`console.warn`.
 */
/** `describeGpu`: anota a placa de vídeo no log (só a janela principal precisa). */
export function installGlobalLogging({ describeGpu = true } = {}) {
  window.addEventListener("error", (ev) => {
    const where = ev.filename ? ` (at ${ev.filename.split("/").pop()}:${ev.lineno})` : "";
    send("error", "app", `Unexpected error: ${describeError(ev.error ?? ev.message)}${where}`);
  });
  window.addEventListener("unhandledrejection", (ev) => {
    send("error", "app", `Unexpected error: ${describeError(ev.reason)}`);
  });

  const original = { error: console.error, warn: console.warn };
  console.error = (...args: unknown[]) => {
    original.error(...args);
    send("error", "app", args.map(describeError).join(" "));
  };
  console.warn = (...args: unknown[]) => {
    original.warn(...args);
    send("warn", "app", args.map(describeError).join(" "));
  };

  // informações úteis para diagnosticar problemas de vídeo
  if (describeGpu) log.info("app", `Graphics card: ${gpuName()}`);
}

function gpuName(): string {
  try {
    const gl = document.createElement("canvas").getContext("webgl");
    const ext = gl?.getExtension("WEBGL_debug_renderer_info");
    return (ext && gl?.getParameter(ext.UNMASKED_RENDERER_WEBGL)) || "unknown";
  } catch {
    return "unknown";
  }
}
