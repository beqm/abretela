<script lang="ts">
  import { onMount } from "svelte";
  import { check } from "@tauri-apps/plugin-updater";
  import { relaunch } from "@tauri-apps/plugin-process";

  let { ondone }: { ondone: () => void } = $props();

  let status = $state("Procurando atualizações...");
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

    status = `Baixando a versão ${update.version}...`;
    try {
      let total = 0;
      let received = 0;
      await update.downloadAndInstall((ev) => {
        if (ev.event === "Started") total = ev.data.contentLength ?? 0;
        if (ev.event === "Progress") {
          received += ev.data.chunkLength;
          if (total) progress = Math.min(100, Math.round((received / total) * 100));
        }
        if (ev.event === "Finished") status = "Instalando...";
      });
      status = "Reiniciando...";
      await relaunch();
    } catch (e) {
      failed = String(e);
    }
  });
</script>

<div class="updater">
  <span class="logo big-logo"></span>
  {#if failed}
    <p>Não foi possível atualizar.</p>
    <p class="hint">{failed}</p>
    <button class="btn primary" onclick={ondone}>Continuar mesmo assim</button>
  {:else}
    <p>{status}</p>
    {#if progress !== null}
      <div class="progress"><div style:width="{progress}%"></div></div>
    {:else}
      <span class="spinner"></span>
    {/if}
  {/if}
</div>
