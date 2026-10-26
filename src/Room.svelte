<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { makeInvite, type RemoteView, type RoomClient } from "./lib/room";
  import { startNativeCapture, type Source } from "./lib/capture";
  import type { Quality } from "./lib/quality";
  import { t } from "./lib/prefs.svelte";
  import SourcePicker from "./SourcePicker.svelte";
  import SettingsModal from "./SettingsModal.svelte";
  import Tile from "./Tile.svelte";
  import Credits from "./Credits.svelte";
  import Icon from "./Icon.svelte";
  import Avatar from "./Avatar.svelte";

  type Props = {
    client: RoomClient;
    code: string;
    password: string;
    isHost: boolean;
    onexit: () => void;
  };
  let { client, code, password, isHost, onexit }: Props = $props();

  // o RoomClient não é reativo; um contador força a releitura
  let tick = $state(0);
  onMount(() => {
    const unsubscribe = client.subscribe(() => tick++);
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
  let picking = $state(false);
  let showSettings = $state(false);
  let focusId = $state<string | null>(null);

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
      me: peers.find((p) => p.isSelf),
      peers,
      channels: peers.filter((p) => p.sharing && !p.isSelf),
      tiles,
      unwatched: peers.filter((p) => p.sharing && !p.isSelf && !p.watching).length,
    };
  });

  // svelte-ignore state_referenced_locally
  const s = client.settings;
  const limits = `${s.minHeight}p–${s.maxHeight}p · ${s.minFps}–${s.maxFps}fps`;

  const focused = $derived(snap.tiles.find((x) => x.id === focusId) ?? null);
  const others = $derived(snap.tiles.filter((x) => x.id !== focused?.id));
  const cols = $derived(Math.ceil(Math.sqrt(snap.tiles.length)));

  function audioFor(id: string) {
    let a = client.volumes.get(id);
    if (!a) client.volumes.set(id, (a = { volume: 1, muted: false }));
    return a;
  }

  function leave() {
    if (isHost && snap.status === "connected" && client.peers.size > 1) {
      if (!confirm(t("hostLeaveConfirm"))) return;
    }
    client.leave();
    if (isHost) invoke("close_room");
    onexit();
  }

  async function startShare(pick: { source: Source; quality: Quality; audio: boolean }) {
    notice = null;
    try {
      const capture = await startNativeCapture(pick.source.id, pick.quality.height, pick.quality.fps, pick.audio);
      const title =
        pick.source.kind === "screen"
          ? t("screenN", { n: pick.source.title })
          : pick.source.app || pick.source.title;
      client.startShare(capture, pick.quality, title);
    } catch (e) {
      notice = t("shareFailed", { e: String(e) });
    } finally {
      picking = false;
    }
  }

  async function copyInvite() {
    await navigator.clipboard.writeText(makeInvite(code, password));
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
      <code title={code}>{code}</code>
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

    <button class="btn small" class:mint={copied} title={t("copyInviteHint")} onclick={copyInvite}>
      <Icon name={copied ? "check" : "copy"} size={16} />
      {copied ? t("copied") : t("copyInvite")}
    </button>

    <div class="spacer"></div>

    <div class="people" title={t("peopleInRoom", { n: snap.peers.length })}>
      <div class="avatar-stack">
        {#each snap.peers.slice(0, 5) as p (p.id)}
          <Avatar name={p.name} size={30} />
        {/each}
      </div>
      <span class="people-count">{snap.peers.length}</span>
    </div>

    <button class="icon-btn" title={t("settings")} onclick={() => (showSettings = true)}>
      <Icon name="settings" size={20} />
    </button>
    <button class="btn small danger-soft" onclick={leave}>
      <Icon name="logout" size={16} />
      {t("leave")}
    </button>
  </header>

  <section class="channels">
    <!-- meu "canal" -->
    {#if snap.sharing && snap.me}
      <div class="channel mine live-on">
        <Avatar name={snap.me.name} size={40} />
        <div class="channel-info">
          <span class="channel-name">{snap.me.name} <span class="muted">({t("you")})</span></span>
          <span class="channel-label">{snap.me.label ?? ""}</span>
        </div>
        <div class="channel-actions">
          <button class="btn small" class:on={snap.me.watching} onclick={() => toggleWatch(snap.me!.id, snap.me!.watching)}>
            {snap.me.watching ? t("stopWatching") : t("preview")}
          </button>
          <button class="btn small danger" onclick={() => client.stopShare()}>{t("closeMyScreen")}</button>
        </div>
      </div>
    {:else}
      <button class="channel open-screen" disabled={snap.status !== "connected"} onclick={() => (picking = true)}>
        <span class="open-icon"><Icon name="plus" size={24} /></span>
        <div class="channel-info">
          <span class="channel-name">{t("openMyScreen")}</span>
          <span class="channel-label">{t("openMyScreenHint")}</span>
        </div>
      </button>
    {/if}

    {#each snap.channels as p (p.id)}
      <div class="channel live-on" class:watching={p.watching}>
        <Avatar name={p.name} size={40} />
        <div class="channel-info">
          <span class="channel-name">{p.name}</span>
          <span class="channel-label" title={p.label ?? ""}>{p.label ?? ""}</span>
        </div>
        <button class="btn small" class:primary={!p.watching} onclick={() => toggleWatch(p.id, p.watching)}>
          {p.watching ? t("stopWatching") : t("watch")}
        </button>
      </div>
    {/each}

    {#if snap.unwatched > 1}
      <button class="btn small watch-all" onclick={() => client.watchAll()}>{t("watchAll")}</button>
    {/if}
  </section>

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
        <p class="muted">{t("emptyBody")}</p>
        <p class="hint">{t("roomQuality", { limits })}{isHost ? ` · ${t("hostHint")}` : ""}</p>
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
          />
        {/each}
      </div>
    {/if}

    <div class="stage-credits"><Credits /></div>
  </main>

  {#if picking}
    <SourcePicker settings={client.settings} onpick={startShare} oncancel={() => (picking = false)} />
  {/if}
  {#if showSettings}
    <SettingsModal onclose={() => (showSettings = false)} />
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
