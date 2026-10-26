<script lang="ts">
  import { onMount } from "svelte";
  import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
  import { getVersion } from "@tauri-apps/api/app";
  import { findUpdate, installUpdate, type Update } from "./lib/update";
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

  // procurar atualizações
  type UpdateState =
    | { kind: "idle" }
    | { kind: "checking" }
    | { kind: "latest" }
    | { kind: "available"; update: Update }
    | { kind: "installing"; phase: "download" | "install"; progress: number | null }
    | { kind: "error"; message: string };
  let version = $state("");
  let upd = $state<UpdateState>({ kind: "idle" });

  async function checkUpdates() {
    upd = { kind: "checking" };
    try {
      const update = await findUpdate(15000);
      upd = update ? { kind: "available", update } : { kind: "latest" };
    } catch (e) {
      upd = { kind: "error", message: String(e) };
    }
  }

  async function updateNow(update: Update) {
    upd = { kind: "installing", phase: "download", progress: null };
    try {
      await installUpdate(update, (phase, progress) => (upd = { kind: "installing", phase, progress }));
    } catch (e) {
      upd = { kind: "error", message: String(e) };
    }
  }

  let autostart = $state(false);
  let autostartReady = $state(false);

  onMount(() => {
    getVersion().then((v) => (version = v));
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

    <section>
      <div class="section-label"><Icon name="refresh" size={16} /> {t("about")}</div>
      <div class="about">
        <img class="about-logo" src="/logo.svg" alt="" />
        <div class="about-info">
          <span class="about-name">AbreTela {version ? `v${version}` : ""}</span>
          <span class="hint">
            {#if upd.kind === "checking"}
              {t("checkingUpdates")}
            {:else if upd.kind === "latest"}
              {t("upToDate")}
            {:else if upd.kind === "available"}
              {t("updateAvailable", { v: upd.update.version })}
            {:else if upd.kind === "installing"}
              {upd.phase === "install" ? t("installing") : t("downloadingUpdate")}
            {:else if upd.kind === "error"}
              <span class="error-text" title={upd.message}>{t("updateCheckFailed")}</span>
            {/if}
          </span>
          {#if upd.kind === "installing" && upd.progress !== null}
            <div class="progress small"><div style:width="{upd.progress}%"></div></div>
          {/if}
        </div>
        {#if upd.kind === "available"}
          {@const update = upd.update}
          <button class="btn small primary" onclick={() => updateNow(update)}>{t("updateNow")}</button>
        {:else}
          <button
            class="btn small"
            disabled={upd.kind === "checking" || upd.kind === "installing"}
            onclick={checkUpdates}
          >
            {#if upd.kind === "checking" || upd.kind === "installing"}<span class="spinner small"></span>{/if}
            {t("checkForUpdates")}
          </button>
        {/if}
      </div>
    </section>
  </div>
</div>
