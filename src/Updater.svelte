<script lang="ts">
  import { onMount } from "svelte";
  import { findUpdate, installUpdate } from "./lib/update";
  import { t } from "./lib/prefs.svelte";

  let { ondone }: { ondone: () => void } = $props();

  let status = $state(t("checkingUpdates"));
  let progress = $state<number | null>(null);
  let failed = $state<string | null>(null);

  onMount(async () => {
    if (import.meta.env.DEV) return ondone();

    let update;
    try {
      update = await findUpdate();
    } catch (e) {
      // sem internet, GitHub fora do ar etc.: abre o app normalmente
      console.warn("[updater]", e);
      return ondone();
    }
    if (!update) return ondone();

    const version = update.version;
    status = t("downloadingVersion", { v: version });
    try {
      await installUpdate(update, (phase, pct) => {
        progress = pct;
        status = phase === "install" ? t("installing") : t("downloadingVersion", { v: version });
      });
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
