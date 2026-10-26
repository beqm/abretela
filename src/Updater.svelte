<script lang="ts">
  import { onMount } from "svelte";
  import { check } from "@tauri-apps/plugin-updater";
  import { relaunch } from "@tauri-apps/plugin-process";
  import { t } from "./lib/prefs.svelte";

  let { ondone }: { ondone: () => void } = $props();

  let status = $state(t("checkingUpdates"));
  let progress = $state<number | null>(null);
  let failed = $state<string | null>(null);

  const withTimeout = <T,>(p: Promise<T>, ms: number) =>
    Promise.race([p, new Promise<never>((_, rej) => setTimeout(() => rej(new Error("timeout")), ms))]);

  onMount(async () => {
    if (import.meta.env.DEV) return ondone();

    let update;
    try {
      update = await withTimeout(check(), 8000);
    } catch (e) {
      // sem internet, GitHub fora do ar etc.: abre o app normalmente
      console.warn("[updater]", e);
      return ondone();
    }
    if (!update) return ondone();

    status = t("downloadingVersion", { v: update.version });
    try {
      let total = 0;
      let received = 0;
      await update.downloadAndInstall((ev) => {
        if (ev.event === "Started") total = ev.data.contentLength ?? 0;
        if (ev.event === "Progress") {
          received += ev.data.chunkLength;
          if (total) progress = Math.min(100, Math.round((received / total) * 100));
        }
        if (ev.event === "Finished") status = t("installing");
      });
      status = t("restarting");
      await relaunch();
    } catch (e) {
      failed = String(e);
    }
  });
</script>

<div class="updater">
  <img class="mascot bob" src="/logo.svg" alt="" />
  {#if failed}
    <p>{t("updateFailed")}</p>
    <p class="hint">{failed}</p>
    <button class="btn primary" onclick={ondone}>{t("continueAnyway")}</button>
  {:else}
    <p class="updater-status">{status}</p>
    {#if progress !== null}
      <div class="progress"><div style:width="{progress}%"></div></div>
    {:else}
      <span class="spinner"></span>
    {/if}
  {/if}
</div>
