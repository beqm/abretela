<script lang="ts">
  import { onMount } from "svelte";
  import { disable, enable, isEnabled } from "@tauri-apps/plugin-autostart";
  import { getVersion } from "@tauri-apps/api/app";
  import { findUpdate, installUpdate, type Update } from "../../lib/update";
  import { LANGUAGES, prefs, t, type Theme } from "../../lib/prefs.svelte";
  import Icon, { type IconName } from "../../ui/Icon.svelte";
  import Toggle from "../../ui/Toggle.svelte";
  import Credits from "../../ui/Credits.svelte";
  import Avatar from "../../ui/Avatar.svelte";

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
            <Toggle
              label={t("nativeSend")}
              hint={t("nativeSendHint")}
              checked={prefs.nativeSend}
              onchange={(v) => (prefs.nativeSend = v)}
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

<style>
  .lang-grid {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 6px;
  }

  .lang {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 38px;
    padding: 0 12px;
    background: var(--surface);
    border: 1px solid var(--line-strong);
    border-radius: var(--r);
    font-weight: 600;
    cursor: pointer;
    text-align: left;
    transition:
      background 0.12s,
      border-color 0.12s;
  }

  .lang:hover {
    background: var(--surface-2);
  }

  .lang.selected {
    border-color: var(--primary);
    background: var(--primary-soft);
    color: var(--primary-text);
  }

  .toggles {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--line);
    border-radius: var(--r);
    overflow: hidden;
  }

  /* linha entre um Toggle e o próximo */
  .toggles > :global(.toggle-row + .toggle-row) {
    border-top: 1px solid var(--line);
  }

  .error-text {
    color: var(--danger);
  }

  .settings-page {
    position: fixed;
    inset: 0;
    z-index: 20;
    display: flex;
    background: var(--bg);
    animation: fade-in 0.12s ease-out;
  }

  .settings-nav {
    width: 232px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 14px 10px;
    background: var(--surface);
    border-right: 1px solid var(--line);
  }

  .settings-back {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 30px;
    padding: 0 10px 0 8px;
    border: none;
    border-radius: var(--r-sm);
    background: none;
    color: var(--muted);
    font-weight: 700;
    font-size: 13px;
    cursor: pointer;
  }

  .settings-back:hover {
    background: var(--surface-2);
    color: var(--text);
  }

  .settings-heading {
    font-size: 20px;
    padding: 14px 10px 10px;
  }

  .settings-nav nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .settings-nav-item {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 38px;
    padding: 0 10px;
    border: none;
    border-radius: var(--r);
    background: none;
    color: var(--muted);
    font-weight: 700;
    text-align: left;
    cursor: pointer;
    transition:
      background 0.12s,
      color 0.12s;
  }

  .settings-nav-item:hover {
    background: var(--surface-2);
    color: var(--text);
  }

  .settings-nav-item.active {
    background: var(--primary-soft);
    color: var(--primary-text);
  }

  .settings-nav-foot {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    font-size: 12px;
    color: var(--muted);
  }

  .settings-nav-foot img {
    width: 22px;
    height: 22px;
  }

  .settings-content {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
  }

  .settings-inner {
    max-width: 640px;
    padding: 40px 40px 32px;
    display: flex;
    flex-direction: column;
    gap: 28px;
  }

  .settings-title {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .settings-title h2 {
    font-size: 24px;
  }

  .settings-group {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .group-label {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 12px;
    font-weight: 800;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--muted);
  }

  .settings-page .lang-grid {
    grid-template-columns: repeat(3, 1fr);
  }

  .settings-page .toggles {
    background: var(--surface);
  }

  .theme-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 12px;
  }

  .theme-card {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 8px;
    background: var(--surface);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-lg);
    cursor: pointer;
    text-align: left;
    transition:
      border-color 0.12s,
      background 0.12s;
  }

  .theme-card:hover {
    background: var(--surface-2);
  }

  .theme-card.selected {
    border-color: var(--primary);
    box-shadow: 0 0 0 1px var(--primary);
  }

  .theme-name {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 4px 2px;
    font-weight: 700;
  }

  .theme-card.selected .theme-name {
    color: var(--primary-text);
  }

  /* miniaturas do app em cada tema */
  .theme-preview {
    --p-bg: #fbf6f0;
    --p-side: #fffdfa;
    --p-line: #ecdfd1;
    --p-tile: #2a1d14;
    display: flex;
    aspect-ratio: 16 / 10;
    border-radius: var(--r-sm);
    overflow: hidden;
    background: var(--p-bg);
    border: 1px solid var(--p-line);
  }

  .theme-preview.dark {
    --p-bg: #17120f;
    --p-side: #201915;
    --p-line: #31271f;
    --p-tile: #362c25;
  }

  .theme-preview.system {
    background: linear-gradient(135deg, #fbf6f0 50%, #17120f 50%);
  }

  .theme-preview.system .tp-side {
    background: #fffdfa;
  }

  .tp-side {
    width: 26%;
    background: var(--p-side);
    border-right: 1px solid var(--p-line);
  }

  .tp-main {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 6%;
    padding: 7%;
  }

  .tp-bar {
    height: 10%;
    width: 55%;
    border-radius: 3px;
    background: var(--primary);
  }

  .tp-tiles {
    flex: 1;
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6%;
  }

  .tp-tiles span {
    border-radius: 4px;
    background: var(--p-tile);
  }

  .theme-preview.system .tp-tiles span {
    background: #2a221d;
  }

  .about-hero {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 18px;
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--r-lg);
  }

  .about-hero img {
    width: 64px;
    height: 64px;
  }

  .about-name {
    font-size: 20px;
    font-weight: 900;
    color: var(--primary);
  }

  .update-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 14px;
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--r);
  }

  .update-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .settings-page :global(.credits) {
    justify-content: flex-start;
  }

  .profile-row {
    display: flex;
    align-items: flex-end;
    gap: 12px;
  }

  .profile-row .field {
    flex: 1;
  }
</style>
