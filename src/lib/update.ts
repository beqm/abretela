// Atualização automática via GitHub Releases (usada na abertura e nas configurações).

import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { isLinux } from "./platform";

export type { Update };

/** A página da última versão, com os instaladores. */
export const RELEASES_URL = "https://github.com/beqm/abretela/releases/latest";

/**
 * O app se atualiza sozinho aqui? No Windows e no Mac, sim. No Linux o app
 * vem num `.deb`, que o atualizador não instala: a pessoa baixa o novo.
 */
export const autoUpdates = () => !isLinux();

/** `a` é uma versão mais nova que `b`? ("0.10.0" > "0.9.1") */
export function isNewer(a: string, b: string): boolean {
  const parts = (v: string) => v.replace(/^v/, "").split(/[.-]/).map((n) => parseInt(n, 10) || 0);
  const [x, y] = [parts(a), parts(b)];
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    if ((x[i] ?? 0) !== (y[i] ?? 0)) return (x[i] ?? 0) > (y[i] ?? 0);
  }
  return false;
}

/** A última versão publicada (sem o "v"), para quem não se atualiza sozinho. */
export async function latestVersion(timeoutMs = 15000): Promise<string> {
  const res = await fetch("https://api.github.com/repos/beqm/abretela/releases/latest", {
    signal: AbortSignal.timeout(timeoutMs),
    headers: { Accept: "application/vnd.github+json" },
  });
  if (!res.ok) throw new Error(`GitHub answered ${res.status}`);
  const { tag_name } = (await res.json()) as { tag_name?: string };
  if (!tag_name) throw new Error("GitHub sent no version");
  return tag_name.replace(/^v/, "");
}

/** Procura uma versão nova; `null` se já está na mais recente. */
export function findUpdate(timeoutMs = 8000): Promise<Update | null> {
  return Promise.race([
    check(),
    new Promise<never>((_, reject) => setTimeout(() => reject(new Error("timeout")), timeoutMs)),
  ]);
}

/** Baixa, instala e reinicia o app. `progress` recebe 0–100 (ou null se o tamanho for desconhecido). */
export async function installUpdate(
  update: Update,
  onProgress: (phase: "download" | "install", progress: number | null) => void,
) {
  let total = 0;
  let received = 0;
  await update.downloadAndInstall((ev) => {
    if (ev.event === "Started") {
      total = ev.data.contentLength ?? 0;
      onProgress("download", total ? 0 : null);
    } else if (ev.event === "Progress") {
      received += ev.data.chunkLength;
      if (total) onProgress("download", Math.min(100, Math.round((received / total) * 100)));
    } else if (ev.event === "Finished") {
      onProgress("install", null);
    }
  });
  await relaunch();
}
