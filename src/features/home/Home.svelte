<script lang="ts">
  import { untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { RoomClient } from "../../lib/room";
  import { WrongPasswordError } from "../../lib/signaling";
  import { parseInvite } from "../../lib/invite";
  import {
    DEFAULT_SETTINGS,
    FPS_OPTIONS,
    HEIGHTS,
    loadJson,
    saveJson,
    type RoomSettings,
  } from "../../lib/quality";
  import { prefs, t } from "../../lib/prefs.svelte";
  import Credits from "../../ui/Credits.svelte";
  import Icon from "../../ui/Icon.svelte";
  import Avatar from "../../ui/Avatar.svelte";
  import SettingsPage from "../settings/SettingsPage.svelte";

  type Props = {
    onenter: (room: { client: RoomClient; room: string; password: string; isHost: boolean }) => void;
    /** Link de convite que abriu o app (abretela://...), ainda não usado. */
    link?: string | null;
    onlinkused?: () => void;
  };
  let { onenter, link = null, onlinkused }: Props = $props();

  const SETTINGS_KEY = "abretela.roomSettings";

  let code = $state("");
  let joinPassword = $state("");
  let roomPassword = $state("");
  let busy = $state<string | null>(null);
  let error = $state<string | null>(null);
  let showOptions = $state(false);
  let showSettings = $state(false);
  let settings = $state<RoomSettings>(loadJson(SETTINGS_KEY, DEFAULT_SETTINGS));

  const trimmedName = $derived(prefs.name);
  // o convite (link) já traz a senha
  const invite = $derived(parseInvite(code));
  const inviteHasPassword = $derived(!!invite?.password);

  // resumo das opções da sala, mostrado só quando fogem do padrão
  const customized = $derived(
    !!roomPassword.trim() ||
      (Object.keys(DEFAULT_SETTINGS) as (keyof RoomSettings)[]).some((k) => settings[k] !== DEFAULT_SETTINGS[k]),
  );
  const summary = $derived(
    [
      roomPassword.trim() ? t("customPassword") : null,
      `${settings.minHeight}p–${settings.maxHeight}p`,
      `${settings.minFps}–${settings.maxFps} fps`,
    ]
      .filter(Boolean)
      .join(" · "),
  );

  // mínimo nunca acima do máximo
  function setSetting(key: keyof RoomSettings, v: number) {
    settings[key] = v;
    if (settings.minHeight > settings.maxHeight) {
      if (key === "minHeight") settings.maxHeight = v;
      else settings.minHeight = v;
    }
    if (settings.minFps > settings.maxFps) {
      if (key === "minFps") settings.maxFps = v;
      else settings.minFps = v;
    }
    saveJson(SETTINGS_KEY, settings);
  }

  // chegou um link de convite: preenche e, se já der, entra direto
  $effect(() => {
    if (!link) return;
    const l = link;
    untrack(() => {
      onlinkused?.();
      showOptions = false;
      code = l;
      error = null;
      if (!busy && parseInvite(l)?.password) joinRoom();
    });
  });

  function errText(e: unknown) {
    if (e instanceof WrongPasswordError) return t("wrongPassword");
    if (e instanceof Error && e.message === "connect-failed") return t("connectFailed");
    return String(e instanceof Error ? e.message : e);
  }

  async function createRoom() {
    error = null;
    busy = t("creatingRoom");
    try {
      const info = await invoke<{ port: number; url: string; password: string }>("create_room", {
        settings: $state.snapshot(settings),
        password: roomPassword.trim() || null,
        hostName: trimmedName,
        lang: prefs.lang,
      });
      const client = new RoomClient(`ws://127.0.0.1:${info.port}/ws`, trimmedName, info.password);
      try {
        await client.connect();
      } catch (e) {
        await invoke("close_room");
        throw e;
      }
      onenter({ client, room: info.url, password: info.password, isHost: true });
    } catch (e) {
      error = errText(e);
      busy = null;
    }
  }

  async function joinRoom(e?: SubmitEvent) {
    e?.preventDefault();
    error = null;
    if (!invite) {
      error = t("invalidCode");
      return;
    }
    const password = invite.password ?? joinPassword.trim();
    if (!password) {
      error = t("enterPassword");
      return;
    }
    busy = t("connecting");
    try {
      const client = new RoomClient(invite.ws, trimmedName, password);
      await client.connect((n) => {
        if (n > 1) busy = t("connectingAttempt", { n });
      });
      onenter({ client, room: invite.room, password, isHost: false });
    } catch (e) {
      error = errText(e);
      busy = null;
    }
  }
</script>

<div class="home">
  <button class="icon-btn home-settings" title={t("settings")} onclick={() => (showSettings = true)}>
    <Icon name="settings" size={22} />
  </button>

  <div class="hero">
    <img class="mascot bob" src="/logo.svg" alt="" />
    <h1 class="wordmark">AbreTela</h1>
    <p class="tagline">{t("tagline")}</p>
  </div>

  <div class="card home-card">
    {#if showOptions}
      <div class="options-head">
        <button class="icon-btn small" title={t("back")} onclick={() => (showOptions = false)}>
          <Icon name="arrowLeft" size={18} />
        </button>
        <h2>{t("roomOptions")}</h2>
      </div>

      <label class="field">
        <span><Icon name="lock" size={13} /> {t("roomPassword")}</span>
        <input bind:value={roomPassword} maxlength="32" placeholder={t("roomPasswordPlaceholder")} spellcheck="false" />
      </label>

      <div class="field">
        <span>{t("allowedQuality")}</span>
        <div class="range-rows">
          <div class="range-row">
            <span class="range-label">{t("resolution")}</span>
            <select value={settings.minHeight} onchange={(e) => setSetting("minHeight", +e.currentTarget.value)}>
              {#each HEIGHTS as h (h)}<option value={h}>{h}p</option>{/each}
            </select>
            <span class="range-to">{t("rangeTo")}</span>
            <select value={settings.maxHeight} onchange={(e) => setSetting("maxHeight", +e.currentTarget.value)}>
              {#each HEIGHTS as h (h)}<option value={h}>{h}p</option>{/each}
            </select>
          </div>
          <div class="range-row">
            <span class="range-label">{t("fps")}</span>
            <select value={settings.minFps} onchange={(e) => setSetting("minFps", +e.currentTarget.value)}>
              {#each FPS_OPTIONS as f (f)}<option value={f}>{f}</option>{/each}
            </select>
            <span class="range-to">{t("rangeTo")}</span>
            <select value={settings.maxFps} onchange={(e) => setSetting("maxFps", +e.currentTarget.value)}>
              {#each FPS_OPTIONS as f (f)}<option value={f}>{f}</option>{/each}
            </select>
          </div>
        </div>
        <p class="hint">{t("qualityHint")}</p>
      </div>

      <button class="btn primary big" onclick={() => (showOptions = false)}>{t("done")}</button>
    {:else}
      <div class="home-identity">
        <Avatar name={trimmedName} size={26} />
        <span class="muted">{t("signedAs")}</span>
        <strong>{trimmedName}</strong>
        <button class="link-btn" disabled={!!busy} onclick={() => (showSettings = true)}>{t("change")}</button>
      </div>

      <div class="create">
        <button class="btn primary big" disabled={!!busy} onclick={createRoom}>
          <Icon name="tv" size={20} />
          {t("createRoom")}
        </button>
        <button
          class="btn big square"
          title={t("roomOptions")}
          disabled={!!busy}
          onclick={() => (showOptions = true)}><Icon name="sliders" size={18} /></button
        >
      </div>
      {#if customized}
        <button class="options-summary" disabled={!!busy} onclick={() => (showOptions = true)}>
          <Icon name="sliders" size={13} />
          {summary}
        </button>
      {/if}

      <div class="divider"><span>{t("or")}</span></div>

      <form class="join" onsubmit={joinRoom}>
        <input bind:value={code} placeholder={t("invitePlaceholder")} disabled={!!busy} spellcheck="false" />
        {#if !inviteHasPassword}
          <input
            class="password"
            type="password"
            bind:value={joinPassword}
            placeholder={t("password")}
            disabled={!!busy}
          />
        {/if}
        <button class="btn" disabled={!code.trim() || !!busy}>{t("join")}</button>
      </form>
    {/if}

    {#if busy}
      <div class="status"><span class="spinner"></span> {busy}</div>
    {/if}
    {#if error}
      <div class="error">{error}</div>
    {/if}
  </div>

  <Credits />
</div>

{#if showSettings}
  <SettingsPage onclose={() => (showSettings = false)} />
{/if}

<style>
  .home {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    /* "safe": se o conteúdo não couber, alinha no topo e deixa rolar */
    justify-content: safe center;
    gap: 28px;
    padding: 32px 16px 20px;
    overflow-y: auto;
  }

  .home-settings {
    position: absolute;
    top: 12px;
    right: 12px;
    z-index: 1;
  }

  .hero {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    text-align: center;
  }

  .wordmark {
    color: var(--primary);
    font-size: 30px;
    font-weight: 900;
    letter-spacing: -0.025em;
  }

  .tagline {
    color: var(--muted);
    max-width: 340px;
    font-size: 14px;
  }

  .home-card {
    width: 100%;
    max-width: 420px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .create {
    display: flex;
    gap: 8px;
  }

  .options-head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: -6px 0 2px -6px;
  }

  .options-head h2 {
    font-size: 16px;
  }

  .range-rows {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .range-row {
    display: grid;
    grid-template-columns: 84px 1fr auto 1fr;
    align-items: center;
    gap: 8px;
  }

  .range-label {
    font-weight: 600;
  }

  .range-to {
    color: var(--muted);
    font-size: 13px;
  }

  .options-summary {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-top: -6px;
    padding: 0;
    border: none;
    background: none;
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
  }

  .options-summary:hover {
    color: var(--text);
  }

  .divider {
    display: flex;
    align-items: center;
    gap: 12px;
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
  }

  .divider::before,
  .divider::after {
    content: "";
    flex: 1;
    height: 1px;
    background: var(--line);
  }

  .join {
    display: flex;
    gap: 8px;
  }

  .join input {
    flex: 1;
    min-width: 0;
  }

  .join input.password {
    flex: 0 0 96px;
  }

  .home-identity {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .home-identity strong {
    font-weight: 800;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .home-identity .link-btn {
    margin-left: auto;
  }

  /* janelas baixas: tela inicial mais compacta */
  @media (max-height: 700px) {
    .home {
      gap: 18px;
      padding: 16px;
    }
    .wordmark {
      font-size: 26px;
    }
    .tagline {
      display: none;
    }
    .home-card {
      padding: 18px;
      gap: 12px;
    }
  }

  /* janela no tamanho mínimo: os créditos ficam só em Configurações > Sobre */
  @media (max-height: 600px) {
    .home > :global(.credits) {
      display: none;
    }
  }
</style>
