<script lang="ts">
  import { untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { RoomClient } from "../../lib/room";
  import { shouldSendNatively } from "../../lib/capture/support";
  import { RoomRefusedError, WrongPasswordError, type RefusedReason } from "../../lib/signaling";
  import { parseInvite, roomLabel } from "../../lib/invite";
  import { forget, remember, sameServer, serverEntries, type ServerEntry } from "../../lib/servers";
  import { selfhost } from "../../lib/selfhost";
  import PasswordModal from "../../ui/PasswordModal.svelte";
  import ServerRail from "./ServerRail.svelte";
  import {
    DEFAULT_SETTINGS,
    FPS_OPTIONS,
    HEIGHTS,
    loadJson,
    saveJson,
    type RoomSettings,
  } from "../../lib/quality";
  import { prefs, t } from "../../lib/prefs.svelte";
  import { TurnRenewal, turnServersForRoom } from "../../lib/turn";
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
  /** Para onde estou entrando: pelo link ou pela lista de servidores. */
  type Target = { room: string; ws: string; password: string | null; owned: ServerEntry["owned"] };
  /** Faltou a senha (ou estava errada): o modal pede, com o erro da última tentativa. */
  let askPassword = $state<{ target: Target; error: string | null } | null>(null);
  let roomPassword = $state("");
  let busy = $state<string | null>(null);
  let error = $state<string | null>(null);
  let showOptions = $state(false);
  let showSettings = $state(false);
  let settings = $state<RoomSettings>(loadJson(SETTINGS_KEY, DEFAULT_SETTINGS));

  const trimmedName = $derived(prefs.name);
  // o link do túnel já traz a senha; o do servidor próprio, não (o modal pede)
  const invite = $derived(parseInvite(code));
  const servers = $derived(serverEntries(prefs.servers, prefs.joinedServers));

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
      if (!busy && parseInvite(l)) joinRoom();
    });
  });

  const REFUSED_TEXT: Record<RefusedReason, Parameters<typeof t>[0]> = {
    banned: "refusedBanned",
    "not-configured": "refusedNotConfigured",
    "too-many-attempts": "refusedTooMany",
    "room-full": "refusedFull",
  };

  function errText(e: unknown) {
    if (e instanceof WrongPasswordError) return t("wrongPassword");
    if (e instanceof RoomRefusedError) return t(REFUSED_TEXT[e.reason]);
    if (e instanceof Error && e.message === "connect-failed") return t("connectFailed");
    return String(e instanceof Error ? e.message : e);
  }

  async function createRoom() {
    error = null;
    busy = t("creatingRoom");
    try {
      // TURN do anfitrião: nunca impede a sala de abrir (sem ele, só conexão direta)
      const turn = $state.snapshot(prefs.turn);
      const lifetime = prefs.turnLifetime;
      const iceServers = await turnServersForRoom(turn, lifetime);
      const info = await invoke<{ port: number; url: string; password: string; hostKey: string }>("create_room", {
        settings: $state.snapshot(settings),
        password: roomPassword.trim() || null,
        hostName: trimmedName,
        lang: prefs.lang,
        iceServers: iceServers.length ? iceServers : null,
      });
      // entra com a chave do anfitrião: pode tirar gente da sala
      const client = new RoomClient(`ws://127.0.0.1:${info.port}/ws`, trimmedName, info.password, {
        nativeSend: shouldSendNatively(prefs.nativeSend),
        admin: info.hostKey,
      });
      try {
        await client.connect();
      } catch (e) {
        await invoke("close_room");
        throw e;
      }
      if (iceServers.length) {
        // senha temporária: uma nova antes de vencer; a sala manda para todos
        const renewal = new TurnRenewal(turn, lifetime, (servers) =>
          invoke("update_room_ice_servers", { iceServers: servers }),
        );
        renewal.start();
        client.onTeardown(() => renewal.stop());
      }
      onenter({ client, room: info.url, password: info.password, isHost: true });
    } catch (e) {
      error = errText(e);
      busy = null;
    }
  }

  /** Entra pelo link. Sem senha no link, usa a guardada do servidor ou pede no modal. */
  function joinRoom() {
    error = null;
    if (!invite) {
      error = t("invalidCode");
      return;
    }
    const known = servers.find((e) => sameServer(e.url, invite.room));
    enter({ room: invite.room, ws: invite.ws, password: invite.password, owned: known?.owned ?? null });
  }

  /** Entra num servidor da lista: pede a senha (ou entra como dono, pela chave). */
  function joinServer(entry: ServerEntry) {
    const url = entry.url.replace(/\/+$/, "");
    enter({ room: url, ws: `${url.replace(/^http/, "ws")}/ws`, password: null, owned: entry.owned });
  }

  async function enter(target: Target, typed?: string) {
    // um clique duplo não pode abrir duas conexões (a segunda viraria um fantasma na sala)
    if (busy) return;
    error = null;
    busy = t("connecting");
    const password = typed ?? target.password;
    // o dono entra pela chave de administrador deste PC, sem senha
    const admin = !typed && target.owned ? await selfhost.adminKey(target.owned).catch(() => null) : null;
    if (!password && !admin) {
      busy = null;
      askPassword = { target, error: null };
      return;
    }
    askPassword = null;
    try {
      const client = new RoomClient(target.ws, trimmedName, password ?? "", {
        nativeSend: shouldSendNatively(prefs.nativeSend),
        admin: admin ?? undefined,
      });
      await client.connect((n) => {
        if (n > 1) busy = t("connectingAttempt", { n });
      });
      // servidor próprio de outra pessoa: fica na lista (sem a senha, que é pedida toda vez)
      if (client.server && !target.owned) {
        prefs.joinedServers = remember(prefs.joinedServers, { url: target.room, name: client.server.name });
      }
      onenter({ client, room: target.room, password: password ?? "", isHost: false });
    } catch (e) {
      busy = null;
      // senha errada (ou a chave de dono não vale mais): pede de novo no modal
      if (e instanceof WrongPasswordError) askPassword = { target: { ...target, owned: null }, error: errText(e) };
      else error = errText(e);
    }
  }

  function forgetServer(entry: ServerEntry) {
    prefs.joinedServers = forget(prefs.joinedServers, entry.url);
  }
</script>

{#if servers.length}
  <ServerRail entries={servers} disabled={!!busy} onjoin={joinServer} onforget={forgetServer} />
{/if}

<div class="home" class:with-rail={servers.length > 0}>
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

      <form
        class="join"
        onsubmit={(e) => {
          e.preventDefault();
          joinRoom();
        }}
      >
        <input bind:value={code} placeholder={t("invitePlaceholder")} disabled={!!busy} spellcheck="false" />
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

{#if askPassword}
  {@const target = askPassword.target}
  <PasswordModal
    room={servers.find((e) => sameServer(e.url, target.room))?.name ?? roomLabel(target.room)}
    error={askPassword.error}
    onsubmit={(password) => enter(target, password)}
    oncancel={() => (askPassword = null)}
  />
{/if}

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

  /* espaço da barra de servidores à esquerda */
  .home.with-rail {
    padding-left: calc(72px + 16px);
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
