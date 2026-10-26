<script lang="ts">
  import { onMount } from "svelte";
  import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
  import { LANGUAGES, prefs, t, type Theme } from "./lib/prefs.svelte";
  import Icon from "./Icon.svelte";
  import Segmented from "./Segmented.svelte";
  import Toggle from "./Toggle.svelte";

  let { onclose }: { onclose: () => void } = $props();

  const themes = $derived([
    { value: "system", label: t("themeSystem") },
    { value: "light", label: t("themeLight") },
    { value: "dark", label: t("themeDark") },
  ]);

  let autostart = $state(false);
  let autostartReady = $state(false);

  onMount(() => {
    isEnabled()
      .then((v) => (autostart = v))
      .catch(() => {})
      .finally(() => (autostartReady = true));
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onclose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  async function setAutostart(v: boolean) {
    autostart = v;
    try {
      await (v ? enable() : disable());
    } catch {
      autostart = await isEnabled().catch(() => false);
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="modal-backdrop" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="modal settings-modal">
    <header class="modal-head">
      <h2>{t("settings")}</h2>
      <button class="icon-btn" title={t("close")} onclick={onclose}><Icon name="x" /></button>
    </header>

    <section>
      <div class="section-label"><Icon name="globe" size={16} /> {t("language")}</div>
      <div class="lang-grid">
        {#each LANGUAGES as l (l.code)}
          <button class="lang" class:selected={prefs.lang === l.code} onclick={() => (prefs.lang = l.code)}>
            {l.name}
            {#if prefs.lang === l.code}<Icon name="check" size={16} />{/if}
          </button>
        {/each}
      </div>
    </section>

    <section>
      <div class="section-label"><Icon name="sun" size={16} /> {t("theme")}</div>
      <Segmented options={themes} value={prefs.theme} onchange={(v) => (prefs.theme = v as Theme)} />
    </section>

    <section>
      <div class="section-label"><Icon name="sliders" size={16} /> {t("behavior")}</div>
      <div class="toggles">
        <Toggle
          label={t("startWithWindows")}
          hint={t("startWithWindowsHint")}
          checked={autostart}
          disabled={!autostartReady}
          onchange={setAutostart}
        />
        <Toggle
          label={t("closeToTray")}
          hint={t("closeToTrayHint")}
          checked={prefs.closeToTray}
          onchange={(v) => (prefs.closeToTray = v)}
        />
        <Toggle
          label={t("soundEffects")}
          hint={t("soundEffectsHint")}
          checked={prefs.sounds}
          onchange={(v) => (prefs.sounds = v)}
        />
      </div>
    </section>
  </div>
</div>
