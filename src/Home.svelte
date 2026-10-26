<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { RoomClient, parseInvite } from "./lib/room";
  import {
    DEFAULT_SETTINGS,
    FPS_OPTIONS,
    HEIGHTS,
    loadJson,
    saveJson,
    type RoomSettings,
  } from "./lib/quality";
  import Segmented from "./Segmented.svelte";
  import Credits from "./Credits.svelte";

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
  let showSettings = $state(false);
  let settings = $state<RoomSettings>(loadJson(SETTINGS_KEY, DEFAULT_SETTINGS));

  const trimmedName = $derived(name.trim());
  // convite "codigo#senha" já traz a senha
  const invite = $derived(parseInvite(code));
  const inviteHasPassword = $derived(!!invite?.password);

  // mínimo nunca acima do máximo
  function setSetting<K extends keyof RoomSettings>(key: K, v: number) {
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

  const errText = (e: unknown) => String(e instanceof Error ? e.message : e);

  async function createRoom() {
    error = null;
    saveJson(NAME_KEY, { name: trimmedName });
    busy = "Criando sala e abrindo o túnel...";
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
      error = "Código inválido.";
      return;
    }
    const password = invite.password ?? joinPassword.trim();
    if (!password) {
      error = "Digite a senha da sala.";
      return;
    }
    saveJson(NAME_KEY, { name: trimmedName });
    busy = "Conectando à sala...";
    try {
      const client = new RoomClient(invite.url, trimmedName, password);
      await client.connect((n) => {
        if (n > 1) busy = `Conectando à sala... (tentativa ${n})`;
      });
      onenter({ client, code: invite.code, password, isHost: false });
    } catch (e) {
      error = errText(e);
      busy = null;
    }
  }
</script>

<div class="home">
  <div class="card">
    <div class="brand">
      <span class="logo"></span>
      <h1>AbreTela</h1>
    </div>
    <p class="muted">Transmita telas, jogos e apps (com áudio) para os amigos.</p>

    <label class="field">
      <span>Seu nome</span>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        bind:value={name}
        maxlength="32"
        placeholder="Como os outros vão te ver"
        disabled={!!busy}
        autofocus
      />
    </label>

    <div class="create">
      <button class="btn primary big" disabled={!trimmedName || !!busy} onclick={createRoom}>
        Criar sala
      </button>
      <button
        class="btn settings-toggle"
        class:open={showSettings}
        title="Configurações da sala"
        disabled={!!busy}
        onclick={() => (showSettings = !showSettings)}>⚙</button
      >
    </div>

    {#if showSettings}
      <div class="room-settings">
        <label class="field">
          <span>Senha da sala</span>
          <input
            bind:value={roomPassword}
            maxlength="32"
            placeholder="Deixe vazio para gerar uma aleatória"
            spellcheck="false"
          />
        </label>
        <div class="settings-title">Qualidade permitida na sala</div>
        <div class="settings-grid">
          <span class="settings-label">Resolução mínima</span>
          <Segmented
            options={HEIGHTS}
            value={settings.minHeight}
            format={(v) => `${v}p`}
            onchange={(v) => setSetting("minHeight", v)}
          />
          <span class="settings-label">Resolução máxima</span>
          <Segmented
            options={HEIGHTS}
            value={settings.maxHeight}
            format={(v) => `${v}p`}
            onchange={(v) => setSetting("maxHeight", v)}
          />
          <span class="settings-label">FPS mínimo</span>
          <Segmented
            options={FPS_OPTIONS}
            value={settings.minFps}
            onchange={(v) => setSetting("minFps", v)}
          />
          <span class="settings-label">FPS máximo</span>
          <Segmented
            options={FPS_OPTIONS}
            value={settings.maxFps}
            onchange={(v) => setSetting("maxFps", v)}
          />
        </div>
        <p class="hint">
          Quem transmitir escolhe a qualidade dentro desses limites. 1080p 60fps exige uns 7 Mbps
          de upload por pessoa assistindo.
        </p>
      </div>
    {/if}

    <div class="divider"><span>ou</span></div>

    <form class="join" onsubmit={joinRoom}>
      <input
        bind:value={code}
        placeholder="Convite ou código da sala"
        disabled={!!busy}
        spellcheck="false"
      />
      {#if !inviteHasPassword}
        <input
          class="password"
          type="password"
          bind:value={joinPassword}
          placeholder="Senha"
          disabled={!!busy}
        />
      {/if}
      <button class="btn" disabled={!trimmedName || !code.trim() || !!busy}>Entrar</button>
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
