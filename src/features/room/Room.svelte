<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { makeInvite, parseInvite, roomLabel, type RemoteView, type RoomClient } from "../../lib/room";
  import { startNativeCapture, type Source } from "../../lib/capture";
  import { describeError, log } from "../../lib/log";
  import type { Quality } from "../../lib/quality";
  import { prefs, t } from "../../lib/prefs.svelte";
  import { sounds } from "../../lib/sounds";
  import SourcePicker from "./SourcePicker.svelte";
  import SettingsPage from "../settings/SettingsPage.svelte";
  import Tile from "./Tile.svelte";
  import Icon from "../../ui/Icon.svelte";
  import Avatar from "../../ui/Avatar.svelte";
  import ConfirmModal from "../../ui/ConfirmModal.svelte";

  type Props = {
    client: RoomClient;
    /** URL https pública da sala. */
    roomUrl: string;
    password: string;
    isHost: boolean;
    onexit: () => void;
    /** Link de convite que chegou com o app aberto, ainda não usado. */
    link?: string | null;
    onlinkdismiss?: () => void;
  };
  let { client, roomUrl, password, isHost, onexit, link = null, onlinkdismiss }: Props = $props();

  // convite para outra sala chegando no meio desta: pergunta antes de sair
  const linkRoom = $derived(link ? parseInvite(link)?.room : null);
  $effect(() => {
    if (link && linkRoom === roomUrl) onlinkdismiss?.(); // já estou nela
  });
  const switching = $derived(!!linkRoom && linkRoom !== roomUrl);

  // o RoomClient não é reativo; um contador força a releitura
  let tick = $state(0);
  onMount(() => {
    const unsubscribe = client.subscribe(() => tick++);
    client.onPeerEvent = (kind) => sounds[kind]();
    sounds.join();
    if (!pipSupported) log.warn("watching", "This WebView2 has no picture-in-picture, so the floating window button is hidden");
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !document.fullscreenElement) focusId = null;
    };
    window.addEventListener("keydown", onKey);
    return () => {
      unsubscribe();
      window.removeEventListener("keydown", onKey);
    };
  });

  let notice = $state<string | null>(null);
  let copied = $state(false);
  let showPassword = $state(false);
  /** Seletor de tela aberto: para começar a transmitir ou para trocar de tela. */
  let pickMode = $state<"start" | "switch" | null>(null);
  let showSettings = $state(false);
  let focusId = $state<string | null>(null);

  // ---------- janela flutuante (Picture-in-Picture) ----------
  // Um <video> próprio, fora dos tiles: trocar de layout recria os tiles, e a
  // janela flutuante fecharia junto.
  let pipVideo: HTMLVideoElement;
  let pipId = $state<string | null>(null);
  const pipSupported = document.pictureInPictureEnabled === true;

  async function togglePopout(id: string) {
    if (pipId === id) {
      await document.exitPictureInPicture().catch(() => {});
      return;
    }
    const stream = client.views.get(id)?.stream;
    if (!stream) return;
    try {
      pipVideo.srcObject = stream;
      await pipVideo.play();
      const win = await pipVideo.requestPictureInPicture();
      pipId = id;
      const report = () => client.setPip(id, win.width, win.height);
      report();
      win.addEventListener("resize", report);
      log.info("watching", `Opened ${client.peers.get(id)?.name ?? id} in a floating window`);
    } catch (e) {
      log.error("watching", `Could not open the floating window: ${describeError(e)}`);
      notice = t("popOutFailed");
    }
  }

  function onLeavePip() {
    if (pipId) log.info("watching", "Closed the floating window");
    pipId = null;
    client.setPip(null);
    pipVideo.srcObject = null;
  }

  // segue a pessoa da janela flutuante: reconectou, troca o vídeo; parou, fecha
  $effect(() => {
    if (!pipId) return;
    const tile = snap.tiles.find((x) => x.id === pipId);
    if (!tile?.view.stream) {
      document.exitPictureInPicture().catch(() => {});
    } else if (pipVideo.srcObject !== tile.view.stream) {
      pipVideo.srcObject = tile.view.stream;
    }
  });

  onDestroy(() => {
    if (document.pictureInPictureElement) document.exitPictureInPicture().catch(() => {});
  });

  const COLLAPSED_KEY = "abretela.sidebarCollapsed";
  let collapsed = $state(localStorage.getItem(COLLAPSED_KEY) === "1");
  function toggleCollapsed() {
    collapsed = !collapsed;
    try {
      localStorage.setItem(COLLAPSED_KEY, collapsed ? "1" : "0");
    } catch {}
  }

  type PeerView = {
    id: string;
    name: string;
    sharing: boolean;
    label?: string | null;
    isSelf: boolean;
    watching: boolean;
  };

  const snap = $derived.by(() => {
    tick;
    const peers: PeerView[] = [...client.peers.values()]
      .map((p) => ({ ...p, isSelf: p.id === client.selfId, watching: client.watching.has(p.id) }))
      .sort((a, b) => (a.isSelf ? -1 : b.isSelf ? 1 : a.name.localeCompare(b.name)));
    const tiles = [...client.watching]
      .filter((id) => client.peers.has(id))
      .map((id) => {
        const isSelf = id === client.selfId;
        const view: RemoteView | undefined = isSelf
          ? { stream: client.localStream, state: "connected" }
          : client.views.get(id);
        const p = client.peers.get(id)!;
        return view && { id, name: p.name, label: p.label, isSelf, view };
      })
      .filter((x) => !!x);
    return {
      status: client.status,
      sharing: !!client.localStream,
      peers,
      tiles,
      unwatched: peers.filter((p) => p.sharing && !p.isSelf && !p.watching).length,
    };
  });

  // svelte-ignore state_referenced_locally
  const s = client.settings;
  const heightRange = s.minHeight === s.maxHeight ? `${s.maxHeight}p` : `${s.minHeight}p–${s.maxHeight}p`;
  const fpsRange = s.minFps === s.maxFps ? `${s.maxFps} fps` : `${s.minFps}–${s.maxFps} fps`;

  // indicador "ao vivo" na bandeja e na barra de tarefas
  $effect(() => {
    invoke("set_live", { live: snap.sharing }).catch(() => {});
  });
  onDestroy(() => {
    client.onPeerEvent = null;
    invoke("set_live", { live: false }).catch(() => {});
  });

  const focused = $derived(snap.tiles.find((x) => x.id === focusId) ?? null);
  const others = $derived(snap.tiles.filter((x) => x.id !== focused?.id));
  const cols = $derived(Math.ceil(Math.sqrt(snap.tiles.length)));

  function audioFor(id: string) {
    let a = client.volumes.get(id);
    if (!a) client.volumes.set(id, (a = { volume: 1, muted: false }));
    return a;
  }

  let confirmingLeave = $state(false);

  function leave() {
    // o anfitrião encerra a sala para todos: pede confirmação se tiver mais gente
    if (isHost && snap.status === "connected" && client.peers.size > 1) {
      confirmingLeave = true;
      return;
    }
    doLeave();
  }

  function doLeave() {
    confirmingLeave = false;
    client.leave();
    if (isHost) invoke("close_room");
    onexit();
  }

  async function startShare(pick: { source: Source; quality: Quality; audio: boolean }) {
    notice = null;
    const switching = pickMode === "switch" && snap.sharing;
    try {
      if (switching) client.releaseCapture();
      const capture = await startNativeCapture(
        pick.source.id,
        pick.quality.height,
        pick.quality.fps,
        pick.audio,
        prefs.hwAccel,
      );
      const title =
        pick.source.kind === "screen"
          ? t("screenN", { n: pick.source.title })
          : pick.source.app || pick.source.title;
      if (switching) client.switchShare(capture, pick.quality, title, prefs.hwAccel);
      else client.startShare(capture, pick.quality, title, prefs.hwAccel);
    } catch (e) {
      log.error("sharing", `Could not ${switching ? "switch the screen" : "start sharing"}: ${describeError(e)}`);
      notice = t("shareFailed", { e: String(e) });
      if (switching) client.stopShare(); // a captura antiga já foi solta
    } finally {
      pickMode = null;
    }
  }

  // ---------- prévia ao passar o mouse em quem está ao vivo ----------
  let sidebar = $state<HTMLElement>();
  let hover = $state<{ id: string; top: number; left: number } | null>(null);
  let hoverTimer: ReturnType<typeof setTimeout> | undefined;

  function hoverPeer(p: PeerView, e: MouseEvent) {
    if (!p.sharing || p.isSelf || p.watching) return;
    const row = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const left = (sidebar?.getBoundingClientRect().right ?? row.right) + 8;
    const top = Math.max(8, Math.min(row.top, window.innerHeight - 240));
    clearTimeout(hoverTimer);
    hoverTimer = setTimeout(() => {
      hover = { id: p.id, top, left };
      client.requestThumb(p.id);
    }, 250);
  }

  function leavePeer() {
    clearTimeout(hoverTimer);
    hover = null;
  }

  const preview = $derived.by(() => {
    tick;
    if (!hover) return null;
    const p = client.peers.get(hover.id);
    if (!p?.sharing || client.watching.has(hover.id)) return null;
    return {
      ...hover,
      name: p.name,
      label: p.label,
      image: client.thumbs.get(hover.id)?.image ?? null,
      loading: client.isThumbLoading(hover.id),
    };
  });

  async function copyInvite() {
    await navigator.clipboard.writeText(makeInvite(roomUrl, password));
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }

  function toggleWatch(id: string, watching: boolean) {
    if (watching) {
      client.unwatch(id);
      if (focusId === id) focusId = null;
    } else {
      client.watch(id);
    }
  }
</script>

<div class="room">
  <header class="topbar">
    <img class="topbar-logo" src="/logo.svg" alt="" />

    <div class="room-pill">
      <span class="pill-label">{t("room")}</span>
      <code title={roomUrl}>{roomLabel(roomUrl)}</code>
      <span class="pill-sep"></span>
      <Icon name="lock" size={14} />
      <code class="pw">{showPassword ? password : "•".repeat(password.length)}</code>
      <button
        class="icon-btn small"
        title={showPassword ? t("hidePassword") : t("showPassword")}
        onclick={() => (showPassword = !showPassword)}
      >
        <Icon name={showPassword ? "eyeOff" : "eye"} size={16} />
      </button>
    </div>

    <button class="btn small" class:success={copied} title={t("copyInviteHint")} onclick={copyInvite}>
      <Icon name={copied ? "check" : "copy"} size={16} />
      {copied ? t("copied") : t("copyInvite")}
    </button>

    <div class="spacer"></div>

    <button class="icon-btn" title={t("settings")} onclick={() => (showSettings = true)}>
      <Icon name="settings" size={20} />
    </button>
    <button class="btn small danger-soft" onclick={leave}>
      <Icon name="logout" size={16} />
      {t("leave")}
    </button>
  </header>

  <div class="room-body">
  <aside class="sidebar" class:collapsed bind:this={sidebar}>
    <div class="sidebar-head">
      {#if !collapsed}
        <span class="sidebar-title">{t("peopleInRoom", { n: snap.peers.length })}</span>
        {#if snap.unwatched > 1}
          <button class="link-btn" onclick={() => client.watchAll()}>{t("watchAll")}</button>
        {/if}
      {/if}
      <button
        class="icon-btn small"
        title={collapsed ? t("expandSidebar") : t("collapseSidebar")}
        onclick={toggleCollapsed}
      >
        <Icon name={collapsed ? "chevronsRight" : "chevronsLeft"} size={18} />
      </button>
    </div>

    <ul class="peer-list">
      {#each snap.peers as p (p.id)}
        <li
          class:watching={p.watching}
          class:live={p.sharing}
          onmouseenter={(e) => hoverPeer(p, e)}
          onmouseleave={leavePeer}
        >
          {#if collapsed}
            <button
              class="peer-avatar-btn"
              disabled={!p.sharing}
              title={p.sharing ? `${p.name} · ${p.label ?? ""}` : p.name}
              onclick={() => toggleWatch(p.id, p.watching)}
            >
              <Avatar name={p.name} size={36} />
              {#if p.sharing}<span class="avatar-live"></span>{/if}
            </button>
          {:else}
            <span class="peer-avatar">
              <Avatar name={p.name} size={34} />
              {#if p.sharing}<span class="avatar-live"></span>{/if}
            </span>
            <div class="peer-info">
              <span class="peer-name">
                {p.name}{#if p.isSelf}{" "}<span class="muted">({t("you")})</span>{/if}
              </span>
              {#if p.sharing && p.label}
                <span class="peer-label" title={p.label}>{p.label}</span>
              {/if}
            </div>
            {#if p.sharing}
              <button
                class="btn small"
                class:soft={!p.watching && !p.isSelf}
                class:ghost={p.watching || p.isSelf}
                onclick={() => toggleWatch(p.id, p.watching)}
              >
                {p.watching ? t("stopWatching") : p.isSelf ? t("preview") : t("watch")}
              </button>
            {/if}
          {/if}
        </li>
      {/each}
    </ul>

    <div class="sidebar-foot">
      {#if snap.sharing}
        <div class="share-actions" class:collapsed>
          <button
            class="btn"
            class:big={!collapsed}
            class:square={collapsed}
            title={t("switchScreen")}
            onclick={() => (pickMode = "switch")}
          >
            <Icon name="refresh" size={18} />
            {#if !collapsed}{t("switchScreen")}{/if}
          </button>
          <button
            class="btn danger"
            class:big={!collapsed}
            class:square={collapsed}
            title={t("closeMyScreen")}
            onclick={() => client.stopShare()}
          >
            <Icon name="x" size={18} />
            {#if !collapsed}{t("stopSharing")}{/if}
          </button>
        </div>
      {:else}
        <button
          class="btn primary"
          class:big={!collapsed}
          class:square={collapsed}
          title={t("openMyScreen")}
          disabled={snap.status !== "connected"}
          onclick={() => (pickMode = "start")}
        >
          <Icon name="plus" size={20} />
          {#if !collapsed}{t("openMyScreen")}{/if}
        </button>
      {/if}
    </div>
  </aside>

  <main class="stage">
    {#if notice}
      <div class="notice">
        <span>{notice}</span>
        <button class="icon-btn small" onclick={() => (notice = null)}><Icon name="x" size={16} /></button>
      </div>
    {/if}

    {#if snap.tiles.length === 0}
      <div class="empty">
        <img class="empty-mascot" src="/logo.svg" alt="" />
        <h2>{t("emptyTitle")}</h2>
        <p class="empty-lead">{t("emptyLead")}</p>

        <div class="empty-actions">
          {#if snap.sharing}
            <button class="action-card" onclick={() => client.watch(client.selfId)}>
              <span class="action-icon live"><Icon name="play" size={20} /></span>
              <span class="action-text">
                <span class="action-title">{t("previewMine")}</span>
                <span class="action-hint">{t("previewMineHint")}</span>
              </span>
            </button>
          {:else}
            <button class="action-card" disabled={snap.status !== "connected"} onclick={() => (pickMode = "start")}>
              <span class="action-icon primary"><Icon name="monitor" size={20} /></span>
              <span class="action-text">
                <span class="action-title">{t("openMyScreen")}</span>
                <span class="action-hint">{t("openMyScreenHint")}</span>
              </span>
            </button>
          {/if}
          {#if snap.unwatched > 0}
            <button class="action-card" onclick={() => client.watchAll()}>
              <span class="action-icon live"><Icon name="tv" size={20} /></span>
              <span class="action-text">
                <span class="action-title">{t("watchAll")}</span>
                <span class="action-hint">{t("liveNow", { n: snap.unwatched })}</span>
              </span>
            </button>
          {:else}
            <button class="action-card" onclick={copyInvite}>
              <span class="action-icon mint"><Icon name={copied ? "check" : "link"} size={20} /></span>
              <span class="action-text">
                <span class="action-title">{copied ? t("copied") : t("emptyInviteTitle")}</span>
                <span class="action-hint">{t("emptyInviteHint")}</span>
              </span>
            </button>
          {/if}
        </div>

        <div class="room-facts">
          <span class="fact" title={t("resolution")}><Icon name="monitor" size={14} /> {heightRange}</span>
          <span class="fact" title={t("fps")}><Icon name="play" size={14} /> {fpsRange}</span>
          <span class="fact"><Icon name="users" size={14} /> {t("peopleInRoom", { n: snap.peers.length })}</span>
          {#if isHost}
            <span class="fact host" title={t("hostHint")}><Icon name="crown" size={14} /> {t("hostBadge")}</span>
          {/if}
        </div>
        <p class="empty-tip">{t("emptyTip")}</p>
      </div>
    {:else if focused}
      <div class="focus-layout">
        <div class="focus-main">
          <Tile
            name={focused.name}
            label={focused.label}
            view={focused.view}
            isSelf={focused.isSelf}
            focused
            audio={audioFor(focused.id)}
            onselect={() => {}}
            onclose={() => toggleWatch(focused.id, true)}
            onretry={() => client.retry(focused.id)}
            onsize={(w, h) => client.setViewSize(focused.id, w, h)}
            popped={pipId === focused.id}
            onpopout={pipSupported && !focused.isSelf ? () => togglePopout(focused.id) : undefined}
          />
          <button class="btn small back-to-grid" onclick={() => (focusId = null)}>
            <Icon name="grid" size={16} />
            {t("seeAll")}
          </button>
        </div>
        {#if others.length > 0}
          <div class="focus-strip">
            {#each others as x (x.id)}
              <Tile
                name={x.name}
                label={x.label}
                view={x.view}
                isSelf={x.isSelf}
                compact
                audio={audioFor(x.id)}
                onselect={() => (focusId = x.id)}
                onclose={() => toggleWatch(x.id, true)}
                onretry={() => client.retry(x.id)}
                onsize={(w, h) => client.setViewSize(x.id, w, h)}
                popped={pipId === x.id}
                onpopout={pipSupported && !x.isSelf ? () => togglePopout(x.id) : undefined}
              />
            {/each}
          </div>
        {/if}
      </div>
    {:else}
      <div class="grid" style:grid-template-columns="repeat({cols}, 1fr)">
        {#each snap.tiles as x (x.id)}
          <Tile
            name={x.name}
            label={x.label}
            view={x.view}
            isSelf={x.isSelf}
            audio={audioFor(x.id)}
            onselect={() => (focusId = x.id)}
            onclose={() => toggleWatch(x.id, true)}
            onretry={() => client.retry(x.id)}
            onsize={(w, h) => client.setViewSize(x.id, w, h)}
            popped={pipId === x.id}
            onpopout={pipSupported && !x.isSelf ? () => togglePopout(x.id) : undefined}
          />
        {/each}
      </div>
    {/if}
  </main>
  </div>

  <!-- svelte-ignore a11y_media_has_caption -->
  <video class="pip-source" bind:this={pipVideo} muted playsinline onleavepictureinpicture={onLeavePip}></video>

  {#if pickMode}
    <SourcePicker
      settings={client.settings}
      switching={pickMode === "switch"}
      onpick={startShare}
      oncancel={() => (pickMode = null)}
    />
  {/if}

  {#if preview}
    <div class="peer-preview" style:top="{preview.top}px" style:left="{preview.left}px">
      <div class="peer-preview-img">
        {#if preview.image}
          <img src={preview.image} alt="" />
        {:else if preview.loading}
          <span class="spinner"></span>
        {:else}
          <span class="muted">{t("noPreview")}</span>
        {/if}
      </div>
      <div class="peer-preview-meta">
        <span class="live-badge"><span class="live-dot"></span>{t("live")}</span>
        <strong>{preview.name}</strong>
      </div>
      {#if preview.label}<div class="hint">{preview.label}</div>{/if}
    </div>
  {/if}
  {#if switching}
    <ConfirmModal
      title={t("switchRoomTitle")}
      message={isHost && client.peers.size > 1 ? t("hostSwitchRoomBody") : t("switchRoomBody")}
      confirmLabel={t("switchRoom")}
      cancelLabel={t("cancel")}
      onconfirm={doLeave}
      oncancel={() => onlinkdismiss?.()}
    />
  {/if}
  {#if confirmingLeave}
    <ConfirmModal
      title={t("hostLeaveTitle")}
      message={t("hostLeaveConfirm")}
      confirmLabel={t("endRoom")}
      cancelLabel={t("cancel")}
      danger
      onconfirm={doLeave}
      oncancel={() => (confirmingLeave = false)}
    />
  {/if}
  {#if showSettings}
    <SettingsPage onclose={() => (showSettings = false)} />
  {/if}

  {#if snap.status === "closed"}
    <div class="modal-backdrop">
      <div class="modal closed-modal">
        <img class="empty-mascot sad" src="/logo.svg" alt="" />
        <h2>{t("roomClosed")}</h2>
        <p class="muted">{t("roomClosedBody")}</p>
        <button
          class="btn primary"
          onclick={() => {
            if (isHost) invoke("close_room");
            onexit();
          }}>{t("backHome")}</button
        >
      </div>
    </div>
  {/if}
</div>

<style>
  .room {
    height: 100%;
    display: flex;
    flex-direction: column;
  }

  .topbar {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 52px;
    padding: 0 12px 0 14px;
    background: var(--surface);
    border-bottom: 1px solid var(--line);
    flex-shrink: 0;
  }

  .topbar-logo {
    width: 28px;
    height: 28px;
    margin-right: 2px;
  }

  .room-pill {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    background: var(--surface-2);
    border-radius: var(--r-sm);
    padding: 0 4px 0 10px;
    min-width: 0;
    color: var(--muted);
  }

  .pill-label {
    font-size: 12px;
    font-weight: 700;
  }

  .room-pill code {
    font-family: "Cascadia Mono", Consolas, ui-monospace, monospace;
    font-size: 12.5px;
    color: var(--text);
    user-select: text;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 260px;
  }

  .room-pill code.pw {
    letter-spacing: 0.06em;
    max-width: 120px;
  }

  .pill-sep {
    width: 1px;
    height: 16px;
    background: var(--line-strong);
  }

  .stage {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
  }

  .notice {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    background: var(--warn-bg);
    color: var(--warn-text);
    border-radius: var(--r);
    padding: 8px 8px 8px 12px;
    font-weight: 600;
    font-size: 13px;
  }

  .notice .icon-btn {
    color: inherit;
  }

  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: safe center;
    text-align: center;
    gap: 6px;
    width: 100%;
    max-width: 560px;
    margin: 0 auto;
    padding: 16px 0;
    overflow-y: auto;
  }

  .empty h2 {
    font-size: 20px;
  }

  .empty-mascot {
    width: 84px;
    height: 84px;
    margin-bottom: 10px;
    animation: bob 2.8s ease-in-out infinite;
  }

  .empty-mascot.sad {
    animation: none;
    filter: grayscale(0.7);
    transform: rotate(-8deg);
  }

  .empty-lead {
    color: var(--muted);
    max-width: 400px;
  }

  .empty-actions {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: 10px;
    width: 100%;
    margin-top: 22px;
  }

  .action-card {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 14px;
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-sm);
    text-align: left;
    cursor: pointer;
    transition:
      border-color 0.12s,
      transform 0.12s,
      box-shadow 0.12s;
  }

  .action-card:hover:not(:disabled) {
    border-color: var(--line-strong);
    box-shadow: var(--shadow);
    transform: translateY(-1px);
  }

  .action-card:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .action-icon {
    width: 42px;
    height: 42px;
    border-radius: 12px;
    display: grid;
    place-items: center;
    flex-shrink: 0;
  }

  .action-icon.primary {
    background: var(--primary-soft);
    color: var(--primary-text);
  }

  .action-icon.mint {
    background: var(--mint-soft);
    color: var(--mint-text);
  }

  .action-icon.live {
    background: var(--live-soft);
    color: var(--live);
  }

  .action-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .action-title {
    font-weight: 800;
  }

  .action-hint {
    font-size: 12.5px;
    color: var(--muted);
  }

  .room-facts {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 6px;
    margin-top: 18px;
  }

  .fact {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 10px;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--muted);
    font-size: 12px;
    font-weight: 700;
  }

  .fact.host {
    background: var(--sun-soft);
    color: var(--sun-text);
  }

  .empty-tip {
    margin-top: 14px;
    font-size: 12px;
    color: var(--muted);
  }

  .grid {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-auto-rows: 1fr;
    gap: 8px;
  }

  /* fonte da janela flutuante: precisa estar no DOM, mas não aparece */
  .pip-source {
    position: fixed;
    width: 1px;
    height: 1px;
    opacity: 0;
    pointer-events: none;
  }

  /* modo foco */

  .focus-layout {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .focus-main {
    flex: 1;
    min-height: 0;
    position: relative;
    display: flex;
  }

  .focus-main :global(.tile) {
    flex: 1;
  }

  .back-to-grid {
    position: absolute;
    top: 8px;
    right: 8px;
  }

  .back-to-grid.btn {
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(8px);
    border-color: transparent;
    color: #fff;
    box-shadow: none;
  }

  .back-to-grid.btn:hover:not(:disabled) {
    background: rgba(0, 0, 0, 0.75);
  }

  .focus-strip {
    height: 112px;
    flex-shrink: 0;
    display: flex;
    gap: 8px;
    overflow-x: auto;
  }

  .focus-strip :global(.tile) {
    width: 199px;
    flex-shrink: 0;
  }

  .closed-modal {
    padding: 28px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    text-align: center;
    max-width: 360px;
  }

  .closed-modal .btn {
    margin-top: 8px;
  }

  .room-body {
    flex: 1;
    min-height: 0;
    display: flex;
  }

  .sidebar {
    width: 260px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border-right: 1px solid var(--line);
    transition: width 0.18s ease;
  }

  .sidebar.collapsed {
    width: 60px;
  }

  .sidebar-head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 8px 6px 16px;
    min-height: 46px;
  }

  .sidebar.collapsed .sidebar-head {
    justify-content: center;
    padding: 10px 0 6px;
  }

  .sidebar-title {
    flex: 1;
    font-size: 12px;
    font-weight: 700;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .peer-list {
    list-style: none;
    margin: 0;
    padding: 0 8px;
    flex: 1;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .peer-list li {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 48px;
    padding: 6px 8px;
    border-radius: var(--r);
    transition: background 0.12s;
  }

  .peer-list li:hover {
    background: var(--surface-2);
  }

  .sidebar.collapsed .peer-list li {
    justify-content: center;
    padding: 4px 0;
    background: none;
  }

  .peer-avatar,
  .peer-avatar-btn {
    position: relative;
    display: grid;
    flex-shrink: 0;
  }

  .peer-avatar-btn {
    background: none;
    border: 2px solid transparent;
    border-radius: 50%;
    padding: 1px;
    cursor: pointer;
  }

  .peer-avatar-btn:disabled {
    cursor: default;
  }

  .peer-list li.watching .peer-avatar-btn {
    border-color: var(--primary);
  }

  .avatar-live {
    position: absolute;
    right: -1px;
    bottom: -1px;
    width: 11px;
    height: 11px;
    border-radius: 50%;
    background: var(--live);
    border: 2px solid var(--surface);
  }

  .peer-info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .peer-name {
    font-weight: 700;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .peer-name .muted {
    font-weight: 500;
  }

  .peer-label {
    font-size: 12px;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .peer-list .btn.small {
    height: 28px;
  }

  .sidebar-foot {
    padding: 12px;
    border-top: 1px solid var(--line);
    display: flex;
    justify-content: center;
  }

  .sidebar.collapsed .sidebar-foot {
    padding: 12px 0;
  }

  .share-actions {
    display: flex;
    gap: 8px;
    width: 100%;
  }

  .share-actions .btn.big {
    flex: 1;
    width: auto;
    padding: 0 10px;
    font-size: 14px;
  }

  .share-actions.collapsed {
    flex-direction: column;
    align-items: center;
  }

  .peer-preview {
    position: fixed;
    z-index: 15;
    width: 260px;
    padding: 8px;
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-lg);
    pointer-events: none;
    animation: pop-in 0.12s ease-out;
  }

  .peer-preview-img {
    aspect-ratio: 16 / 9;
    border-radius: var(--r-sm);
    background: var(--stage);
    display: grid;
    place-items: center;
    overflow: hidden;
    font-size: 12px;
  }

  .peer-preview-img img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  .peer-preview-img .spinner {
    border-color: rgba(255, 255, 255, 0.15);
    border-top-color: rgba(255, 255, 255, 0.7);
  }

  .peer-preview-meta {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 8px 2px 0;
    min-width: 0;
  }

  .peer-preview-meta strong {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .peer-preview .hint {
    margin: 2px 2px 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
