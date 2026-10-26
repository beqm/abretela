<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { RoomClient, WrongPasswordError, parseInvite } from "./lib/room";
  import {
    DEFAULT_SETTINGS,
    FPS_OPTIONS,
    HEIGHTS,
    loadJson,
    saveJson,
    type RoomSettings,
  } from "./lib/quality";
  import { t } from "./lib/prefs.svelte";
  import Segmented from "./Segmented.svelte";
  import Credits from "./Credits.svelte";
  import Icon from "./Icon.svelte";
  import SettingsModal from "./SettingsModal.svelte";

  type Props = {
    onenter: (room: { client: RoomClient; code: string; password: string; isHost: boolean }) => void;
  };
  let { onenter }: Props = $props();

  const NAME_KEY = "abretela.name";
  const SETTINGS_KEY = "abretela.roomSettings";

  let name = $state(loadJson(NAME_KEY, { name: "" }).name);
  let code = $state("");
  let joinPassword = $state("");
  let roomPassword = $state("");
  let busy = $state<string | null>(null);
  let error = $state<string | null>(null);
  let showOptions = $state(false);
  let showSettings = $state(false);
  let settings = $state<RoomSettings>(loadJson(SETTINGS_KEY, DEFAULT_SETTINGS));

  const trimmedName = $derived(name.trim());
  // convite "codigo#senha" já traz a senha
  const invite = $derived(parseInvite(code));
  const inviteHasPassword = $derived(!!invite?.password);

  const heightOpts = HEIGHTS.map((h) => ({ value: h as number, label: `${h}p` }));
  const fpsOpts = FPS_OPTIONS.map((f) => ({ value: f as number, label: String(f) }));

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

  function errText(e: unknown) {
    if (e instanceof WrongPasswordError) return t("wrongPassword");
    if (e instanceof Error && e.message === "connect-failed") return t("connectFailed");
    return String(e instanceof Error ? e.message : e);
  }

  async function createRoom() {
    error = null;
    saveJson(NAME_KEY, { name: trimmedName });
    busy = t("creatingRoom");
    try {
      const info = await invoke<{ port: number; code: string; password: string }>("create_room", {
        settings: $state.snapshot(settings),
        password: roomPassword.trim() || null,
      });
      const client = new RoomClient(`ws://127.0.0.1:${info.port}/ws`, trimmedName, info.password);
      try {
        await client.connect();
      } catch (e) {
        await invoke("close_room");
        throw e;
      }
      onenter({ client, code: info.code, password: info.password, isHost: true });
    } catch (e) {
      error = errText(e);
      busy = null;
    }
  }

  async function joinRoom(e: SubmitEvent) {
    e.preventDefault();
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
    saveJson(NAME_KEY, { name: trimmedName });
    busy = t("connecting");
    try {
      const client = new RoomClient(invite.url, trimmedName, password);
      await client.connect((n) => {
        if (n > 1) busy = t("connectingAttempt", { n });
      });
      onenter({ client, code: invite.code, password, isHost: false });
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
    <img class="mascot" src="/logo.svg" alt="" />
    <h1 class="wordmark">AbreTela</h1>
    <p class="tagline">{t("tagline")}</p>
  </div>

  <div class="card home-card">
    <label class="field">
      <span>{t("yourName")}</span>
      <!-- svelte-ignore a11y_autofocus -->
      <input bind:value={name} maxlength="32" placeholder={t("namePlaceholder")} disabled={!!busy} autofocus />
    </label>

    <div class="create">
      <button class="btn primary big" disabled={!trimmedName || !!busy} onclick={createRoom}>
        <Icon name="tv" size={20} />
        {t("createRoom")}
      </button>
      <button
        class="btn square"
        class:on={showOptions}
        title={t("roomOptions")}
        disabled={!!busy}
        onclick={() => (showOptions = !showOptions)}><Icon name="sliders" size={20} /></button
      >
    </div>

    {#if showOptions}
      <div class="room-options">
        <label class="field">
          <span><Icon name="lock" size={13} /> {t("roomPassword")}</span>
          <input
            bind:value={roomPassword}
            maxlength="32"
            placeholder={t("roomPasswordPlaceholder")}
            spellcheck="false"
          />
        </label>
        <div class="options-title">{t("allowedQuality")}</div>
        <div class="options-grid">
          <span class="opt-label">{t("minResolution")}</span>
          <Segmented options={heightOpts} value={settings.minHeight} onchange={(v) => setSetting("minHeight", v)} />
          <span class="opt-label">{t("maxResolution")}</span>
          <Segmented options={heightOpts} value={settings.maxHeight} onchange={(v) => setSetting("maxHeight", v)} />
          <span class="opt-label">{t("minFps")}</span>
          <Segmented options={fpsOpts} value={settings.minFps} onchange={(v) => setSetting("minFps", v)} />
          <span class="opt-label">{t("maxFps")}</span>
          <Segmented options={fpsOpts} value={settings.maxFps} onchange={(v) => setSetting("maxFps", v)} />
        </div>
        <p class="hint">{t("qualityHint")}</p>
      </div>
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
      <button class="btn mint" disabled={!trimmedName || !code.trim() || !!busy}>{t("join")}</button>
    </form>

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
  <SettingsModal onclose={() => (showSettings = false)} />
{/if}
