// Atualização automática via GitHub Releases (usada na abertura e nas configurações).

import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export type { Update };

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
