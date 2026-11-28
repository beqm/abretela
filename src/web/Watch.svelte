<script lang="ts">
  // Assistir pelo navegador (ROADMAP, seção 5): nome (e a senha, se o link não
  // trouxe), a lista de quem está na sala e as telas abertas. Usa o mesmo
  // RoomClient e o mesmo Tile do app; não transmite nada.
  import { onMount } from "svelte";
  import { RoomClient } from "../lib/room";
  import { RoomRefusedError, WrongPasswordError, type RefusedReason } from "../lib/signaling";
  import { serverInfo } from "../lib/selfhost";
  import { prefs, t, type MessageKey } from "../lib/prefs.svelte";
  import { isLinux, isMac } from "../lib/platform";
  import { appLink, DOWNLOAD_URL, isMobile, rememberedApp, roomFromPage, tryOpenApp } from "./page";
  import logoSvg from "../../public/logo.svg?raw";
  import Tile from "../features/room/Tile.svelte";
  import Avatar from "../ui/Avatar.svelte";
  import Icon from "../ui/Icon.svelte";

  const page = roomFromPage(location);

  let roomName = $state(page.label);
  let counts = $state<{ people: number; live: number } | null>(null);
  let name = $state(prefs.name);
  let password = $state(page.password ?? "");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let client = $state<RoomClient | null>(null);
  let tick = $state(0);

  const logo = `data:image/svg+xml,${encodeURIComponent(logoSvg)}`;
  /** No computador o app existe: abrir nele é o caminho principal. */
  const desktop = !isMobile(navigator.userAgent);
  // o botão de download fala do sistema de quem abriu a página
  const downloadLabel: MessageKey = isMac() ? "webDownloadMac" : isLinux() ? "webDownloadLinux" : "webDownloadWin";
  /** Abrir no app: tentando, abriu, ou demorou (talvez não tenha o app). */
  let app = $state<"idle" | "trying" | "opened" | "slow">("idle");
  let stopTrying: (() => void) | null = null;

  function openApp() {
    stopTrying?.();
    app = "trying";
    stopTrying = tryOpenApp(appLink(page), {
      opened: () => (app = "opened"),
      slow: () => (app = "slow"),
    });
  }

  const REFUSED: Record<RefusedReason, MessageKey> = {
    banned: "refusedBanned",
    "not-configured": "refusedNotConfigured",
    "too-many-attempts": "refusedTooMany",
    "room-full": "refusedFull",
  };

  function errText(e: unknown) {
    if (e instanceof WrongPasswordError) return t("wrongPassword");
    if (e instanceof RoomRefusedError) return t(REFUSED[e.reason]);
    if (e instanceof Error && e.message === "connect-failed") return t("connectFailed");
    return String(e instanceof Error ? e.message : e);
  }

  async function join(e?: SubmitEvent) {
    e?.preventDefault();
    if (!name.trim() || !password.trim() || busy) return;
    busy = true;
    error = null;
    prefs.name = name.trim();
    const c = new RoomClient(page.ws, name.trim(), password.trim(), { web: true });
    c.subscribe(() => tick++);
    try {
      await c.connect();
      client = c;
      if (c.server) roomName = c.server.name;
    } catch (err) {
      c.leave();
      error = errText(err);
    } finally {
      busy = false;
    }
  }

  function leave() {
    client?.leave();
    client = null;
    closePip();
  }

  const snap = $derived.by(() => {
    tick;
    const c = client;
    if (!c) return null;
    const peers = [...c.peers.values()]
      .map((p) => ({ ...p, isSelf: p.id === c.selfId, watching: c.watching.has(p.id) }))
      .sort((a, b) => Number(b.sharing) - Number(a.sharing) || (a.isSelf ? -1 : b.isSelf ? 1 : a.name.localeCompare(b.name)));
    const tiles = [...c.watching]
      .filter((id) => c.peers.has(id) && c.views.has(id))
      .map((id) => ({ id, name: c.peers.get(id)!.name, label: c.peers.get(id)!.label, view: c.views.get(id)! }));
    return {
      status: c.status,
      kicked: c.kicked,
      server: c.server ? { ...c.server } : null,
      peers,
      tiles,
      live: peers.filter((p) => p.sharing && !p.isSelf),
    };
  });

  function audioFor(id: string) {
    let a = client!.volumes.get(id);
    if (!a) {
      a = { volume: 1, muted: false };
      client!.volumes.set(id, a);
    }
    return a;
  }

  function toggleWatch(id: string, watching: boolean) {
    if (!client) return;
    if (watching) client.unwatch(id);
    else client.watch(id);
  }

  // janela flutuante do próprio navegador (Picture-in-Picture), uma de cada vez
  let pipVideo: HTMLVideoElement;
  let pipId = $state<string | null>(null);
  const pipSupported = typeof document !== "undefined" && document.pictureInPictureEnabled === true;

  async function togglePip(id: string, stream: MediaStream | null) {
    if (pipId === id) return closePip();
    if (!stream) return;
    try {
      pipVideo.srcObject = stream;
      await pipVideo.play();
      const win = await pipVideo.requestPictureInPicture();
      pipId = id;
      client?.setPip(id, win.width, win.height);
    } catch {
      pipId = null;
    }
  }

  function closePip() {
    if (document.pictureInPictureElement) document.exitPictureInPicture().catch(() => {});
    pipId = null;
    client?.setPip(null);
  }

  onMount(() => {
    // este navegador já abriu o app antes: tenta de novo sozinho
    if (desktop && rememberedApp()) openApp();
    // servidor próprio: nome da sala e quem está lá antes de entrar (o túnel não tem isso)
    serverInfo(page.room).then((info) => {
      if (!info) return;
      roomName = info.name;
      counts = { people: info.people, live: info.live };
    });
    const onLeavePip = () => {
      pipId = null;
      client?.setPip(null);
    };
    pipVideo?.addEventListener("leavepictureinpicture", onLeavePip);
    return () => {
      stopTrying?.();
      pipVideo?.removeEventListener("leavepictureinpicture", onLeavePip);
    };
  });
</script>

<video bind:this={pipVideo} class="pip-video" muted playsinline></video>

{#if !client || !snap}
  <main class="intro">
    <div class="card intro-card">
      <div class="brand"><img src={logo} alt="" /> AbreTela</div>
      <h1>{roomName}</h1>
      {#if counts}
        <span class="presence" class:live={counts.live > 0}>
          <i></i>{t("selfhostPeople", { people: counts.people, live: counts.live })}
        </span>
      {/if}

      {#if app === "opened"}
        <div class="opened">
          <Icon name="check" size={22} />
          <strong>{t("webOpened")}</strong>
          <span class="muted">{t("webOpenedHint")}</span>
          <button class="link-btn" onclick={() => (app = "idle")}>{t("webWatchAnyway")}</button>
        </div>
      {:else}
        {#if desktop}
          <button class="btn primary big" onclick={openApp}>
            {#if app === "trying"}<span class="spinner small"></span>{:else}<Icon name="tv" size={18} />{/if}
            {t("webOpenAppTitle")}
          </button>
          {#if app === "slow"}
            <p class="hint warn">
              {t("webNotOpened")}
              <a href={DOWNLOAD_URL} target="_blank" rel="noopener">{t(downloadLabel)}</a>
            </p>
          {:else}
            <p class="hint center">{t("webOpenAppHint")}</p>
          {/if}
          <div class="divider"><span>{t("webOrBrowser")}</span></div>
        {/if}

        <form class="form" onsubmit={join}>
          <label class="field">
            <span>{t("webYourName")}</span>
            <input bind:value={name} maxlength="32" autocomplete="nickname" disabled={busy} />
          </label>
          {#if !page.password}
            <label class="field">
              <span>{t("password")}</span>
              <input type="password" bind:value={password} maxlength="64" autocomplete="off" disabled={busy} />
            </label>
          {/if}
          <button
            class="btn big"
            class:primary={!desktop}
            disabled={busy || !name.trim() || !password.trim()}
          >
            {#if busy}<span class="spinner small"></span>{:else}<Icon name="play" size={16} />{/if}
            {busy ? t("connecting") : t("webWatchHere")}
          </button>
          {#if error}<p class="error">{error}</p>{/if}
        </form>
      {/if}

      <p class="foot">
        {#if desktop}
          {t("webNoApp")} <a href={DOWNLOAD_URL} target="_blank" rel="noopener">{t(downloadLabel)}</a>
        {:else}
          {t("webMobileHint")}
        {/if}
      </p>
    </div>
  </main>
{:else}
  <div class="watch">
    <header class="bar">
      <Icon name="tv" size={18} />
      <strong class="room-name" title={page.room}>{roomName}</strong>
      <span class="muted count">{t("peopleInRoom", { n: snap.peers.length })}</span>
      <span class="spacer"></span>
      <button class="btn small danger-soft" onclick={leave}><Icon name="logout" size={15} /> {t("leave")}</button>
    </header>

    <div class="body">
      <aside class="people">
        {#if snap.live.length > 1}
          <button class="link-btn" onclick={() => client?.watchAll()}>{t("watchAll")}</button>
        {/if}
        <ul>
          {#each snap.peers as p (p.id)}
            <li class:live={p.sharing}>
              <Avatar name={p.name} size={30} />
              <div class="who">
                <span class="name">
                  {p.name}
                  {#if p.owner}<span class="crown"><Icon name="crown" size={12} /></span>{/if}
                  {#if p.web}<span class="web" title={t("webViaBrowser")}><Icon name="globe" size={12} /></span>{/if}
                  {#if p.isSelf}<span class="muted">({t("you")})</span>{/if}
                </span>
                {#if p.sharing && p.label}<span class="label">{p.label}</span>{/if}
              </div>
              {#if p.sharing && !p.isSelf}
                <button class="btn small" class:primary={!p.watching} onclick={() => toggleWatch(p.id, p.watching)}>
                  {p.watching ? t("stopWatching") : t("watch")}
                </button>
              {/if}
            </li>
          {/each}
        </ul>
        <p class="hint share-hint">{t("webShareHint")}</p>
      </aside>

      <main class="stage" class:single={snap.tiles.length === 1}>
        {#each snap.tiles as tile (tile.id)}
          <Tile
            name={tile.name}
            label={tile.label}
            view={tile.view}
            isSelf={false}
            audio={audioFor(tile.id)}
            onselect={() => {}}
            onclose={() => toggleWatch(tile.id, true)}
            onretry={() => client?.retry(tile.id)}
            onsize={(w, h) => client?.setViewSize(tile.id, w, h)}
            onpopout={pipSupported ? () => togglePip(tile.id, tile.view.stream) : undefined}
            popped={pipId === tile.id}
          />
        {:else}
          <div class="empty">
            <Icon name="monitor" size={36} />
            <p>{snap.live.length ? t("webPickHint") : t("webNobodyLive")}</p>
          </div>
        {/each}
      </main>
    </div>

    {#if snap.status === "closed"}
      <div class="modal-backdrop">
        <div class="modal closed">
          {#if snap.kicked}
            <h2>{t("kickedTitle")}</h2>
            <p class="muted">{t(snap.kicked.banned ? "kickedBannedBody" : "kickedBody")}</p>
          {:else if snap.server}
            <h2>{t("serverLostTitle")}</h2>
            <p class="muted">{t("webLostBody")}</p>
          {:else}
            <h2>{t("roomClosed")}</h2>
            <p class="muted">{t("roomClosedBody")}</p>
          {/if}
          <button class="btn primary" onclick={leave}>{t("webBack")}</button>
        </div>
      </div>
    {/if}
  </div>
{/if}

<style>
  :global(html),
  :global(body),
  :global(#root) {
    height: 100%;
  }
  .pip-video {
    position: fixed;
    width: 1px;
    height: 1px;
    opacity: 0;
    pointer-events: none;
  }

  /* ---------- entrar ---------- */
  .intro {
    min-height: 100%;
    display: grid;
    place-items: center;
    padding: 24px 16px;
  }
  .intro-card {
    width: min(400px, 100%);
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 14px;
    padding: 28px 24px 20px;
  }
  .brand {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    color: var(--primary);
    font-weight: 900;
    font-size: 15px;
  }
  .brand img {
    width: 34px;
    height: 34px;
  }
  h1 {
    font-size: 26px;
    font-weight: 900;
    margin: 0;
    text-align: center;
    overflow-wrap: anywhere;
  }
  .presence {
    align-self: center;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 4px 12px;
    border-radius: 999px;
    background: var(--mint-soft);
    color: var(--mint-text);
    font-size: 13px;
    font-weight: 700;
  }
  .presence i {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: currentColor;
  }
  .presence.live {
    background: var(--live-soft);
    color: var(--live);
  }
  .intro-card p {
    margin: 0;
  }
  .hint.center {
    text-align: center;
  }
  .hint.warn {
    color: var(--warn-text);
    text-align: center;
  }
  .hint a,
  .foot a {
    color: var(--primary-text);
    font-weight: 700;
  }
  .divider {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--muted);
    font-size: 12px;
  }
  .divider::before,
  .divider::after {
    content: "";
    flex: 1;
    height: 1px;
    background: var(--line);
  }
  .form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .opened {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 18px 12px;
    border-radius: var(--r);
    background: var(--mint-soft);
    color: var(--mint-text);
    text-align: center;
  }
  .opened .muted {
    color: var(--text);
    opacity: 0.75;
  }
  .error {
    color: var(--danger);
    text-align: center;
  }
  .foot {
    text-align: center;
    font-size: 13px;
    color: var(--muted);
    padding-top: 12px;
    border-top: 1px solid var(--line);
  }

  /* ---------- sala ---------- */
  .watch {
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--line);
    color: var(--primary);
  }
  .room-name {
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    white-space: nowrap;
  }
  .spacer {
    flex: 1;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .people {
    width: 270px;
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
    border-right: 1px solid var(--line);
    overflow-y: auto;
  }
  .people ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .people li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    border-radius: var(--r-sm);
  }
  .people li.live {
    background: var(--live-soft);
  }
  .who {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .name {
    font-weight: 700;
    display: flex;
    align-items: center;
    gap: 5px;
    overflow: hidden;
    white-space: nowrap;
  }
  .label {
    font-size: 12px;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .crown {
    color: var(--sun-text);
    display: inline-flex;
  }
  .web {
    color: var(--muted);
    display: inline-flex;
  }
  .share-hint {
    margin-top: auto;
  }
  .stage {
    flex: 1;
    min-width: 0;
    padding: 12px;
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
    grid-auto-rows: minmax(200px, 1fr);
    gap: 12px;
    overflow-y: auto;
  }
  .stage.single {
    grid-template-columns: 1fr;
  }
  .empty {
    grid-column: 1 / -1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    color: var(--muted);
    text-align: center;
  }
  .closed {
    width: min(380px, 100%);
    padding: 24px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    text-align: center;
  }

  /* celular: a lista vai para cima, as telas embaixo */
  @media (max-width: 720px) {
    .body {
      flex-direction: column;
    }
    .people {
      width: auto;
      max-height: 40%;
      border-right: none;
      border-bottom: 1px solid var(--line);
    }
    .share-hint {
      display: none;
    }
    .stage {
      grid-template-columns: 1fr;
      grid-auto-rows: minmax(180px, auto);
    }
  }
</style>
