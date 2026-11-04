<script lang="ts">
  import { onMount } from "svelte";
  import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
  import { getVersion } from "@tauri-apps/api/app";
  import { findUpdate, installUpdate, type Update } from "./lib/update";
  import { LANGUAGES, prefs, t, type Theme } from "./lib/prefs.svelte";
  import Icon, { type IconName } from "./Icon.svelte";
  import Toggle from "./Toggle.svelte";
  import Credits from "./Credits.svelte";
  import Avatar from "./Avatar.svelte";

  let { onclose }: { onclose: () => void } = $props();

  // nome: salva ao sair do campo; vazio volta ao anterior
  let nameDraft = $state(prefs.name);
  function saveName() {
    if (nameDraft.trim()) prefs.name = nameDraft;
    nameDraft = prefs.name;
  }

  type Section = "general" | "appearance" | "about";
  let section = $state<Section>("general");

  const sections = $derived<{ id: Section; icon: IconName; label: string; desc: string }[]>([
    { id: "general", icon: "sliders", label: t("general"), desc: t("generalDesc") },
    { id: "appearance", icon: "sun", label: t("appearance"), desc: t("appearanceDesc") },
    { id: "about", icon: "info", label: t("about"), desc: t("aboutDesc") },
  ]);
  const current = $derived(sections.find((s) => s.id === section)!);

  const themes = $derived<{ value: Theme; label: string }[]>([
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
    getVersion()
      .then((v) => (version = v))
      .catch(() => {});
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

<div class="settings-page">
  <aside class="settings-nav">
    <button class="settings-back" onclick={onclose}>
      <Icon name="arrowLeft" size={16} />
      {t("back")}
    </button>
    <h1 class="settings-heading">{t("settings")}</h1>
    <nav>
      {#each sections as s (s.id)}
        <button class="settings-nav-item" class:active={section === s.id} onclick={() => (section = s.id)}>
          <Icon name={s.icon} size={17} />
          {s.label}
        </button>
      {/each}
    </nav>
    <div class="spacer"></div>
    <div class="settings-nav-foot">
      <img src="/logo.svg" alt="" />
      <span>AbreTela{version ? ` v${version}` : ""}</span>
    </div>
  </aside>

  <main class="settings-content">
    <div class="settings-inner">
      <header class="settings-title">
        <h2>{current.label}</h2>
        <p class="muted">{current.desc}</p>
      </header>

      {#if section === "general"}
        <section class="settings-group">
          <div class="group-label"><Icon name="users" size={15} /> {t("profile")}</div>
          <div class="profile-row">
            <Avatar name={nameDraft.trim() || prefs.name} size={40} />
            <label class="field">
              <span>{t("yourName")}</span>
              <input
                bind:value={nameDraft}
                maxlength="32"
                placeholder={t("namePlaceholder")}
                spellcheck="false"
                onblur={saveName}
                onkeydown={(e) => e.key === "Enter" && e.currentTarget.blur()}
              />
            </label>
          </div>
          <p class="hint">{t("nameHint")}</p>
        </section>

        <section class="settings-group">
          <div class="group-label"><Icon name="globe" size={15} /> {t("language")}</div>
          <div class="lang-grid">
            {#each LANGUAGES as l (l.code)}
              <button class="lang" class:selected={prefs.lang === l.code} onclick={() => (prefs.lang = l.code)}>
                {l.name}
                {#if prefs.lang === l.code}<Icon name="check" size={16} />{/if}
              </button>
            {/each}
          </div>
        </section>

        <section class="settings-group">
          <div class="group-label"><Icon name="monitor" size={15} /> {t("behavior")}</div>
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

        <section class="settings-group">
          <div class="group-label"><Icon name="tv" size={15} /> {t("performance")}</div>
          <div class="toggles">
            <Toggle
              label={t("hwAccel")}
              hint={t("hwAccelHint")}
              checked={prefs.hwAccel}
              onchange={(v) => (prefs.hwAccel = v)}
            />
          </div>
        </section>
      {:else if section === "appearance"}
        <section class="settings-group">
          <div class="group-label"><Icon name="sun" size={15} /> {t("theme")}</div>
          <div class="theme-grid">
            {#each themes as th (th.value)}
              <button
                class="theme-card"
                class:selected={prefs.theme === th.value}
                onclick={() => (prefs.theme = th.value)}
              >
                <span class="theme-preview {th.value}">
                  <span class="tp-side"></span>
                  <span class="tp-main">
                    <span class="tp-bar"></span>
                    <span class="tp-tiles"><span></span><span></span></span>
                  </span>
                </span>
                <span class="theme-name">
                  {th.label}
                  {#if prefs.theme === th.value}<Icon name="check" size={15} />{/if}
                </span>
              </button>
            {/each}
          </div>
        </section>
      {:else}
        <section class="about-hero">
          <img src="/logo.svg" alt="" />
          <div>
            <div class="about-name">AbreTela</div>
            <div class="muted">{version ? `v${version}` : ""}</div>
          </div>
        </section>

        <section class="settings-group">
          <div class="group-label"><Icon name="refresh" size={15} /> {t("checkForUpdates")}</div>
          <div class="update-row">
            <div class="update-text">
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
              {:else}
                <span class="muted">AbreTela{version ? ` v${version}` : ""}</span>
              {/if}
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

        <Credits />
      {/if}
    </div>
  </main>
</div>
