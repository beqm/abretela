<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { makeInvite, type RemoteView, type RoomClient } from "./lib/room";
  import { startNativeCapture, type Source } from "./lib/capture";
  import type { Quality } from "./lib/quality";
  import SourcePicker from "./SourcePicker.svelte";
  import Tile from "./Tile.svelte";

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
  let focusId = $state<string | null>(null);

  type Snapshot = {
    status: RoomClient["status"];
    sharing: boolean;
    peers: { id: string; name: string; sharing: boolean; label?: string | null; isSelf: boolean; watching: boolean }[];
    tiles: { id: string; name: string; label?: string | null; isSelf: boolean; view: RemoteView }[];
    liveOthers: number;
  };

  const snap: Snapshot = $derived.by(() => {
    tick;
    const peers = [...client.peers.values()]
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
      .filter((t) => !!t);
    return {
      status: client.status,
      sharing: !!client.localStream,
      peers,
      tiles,
      liveOthers: peers.filter((p) => p.sharing && !p.isSelf && !p.watching).length,
    };
  });

  // svelte-ignore state_referenced_locally
  const s = client.settings;
  const limits = `${s.minHeight}p–${s.maxHeight}p · ${s.minFps}–${s.maxFps}fps`;

  const focused = $derived(snap.tiles.find((t) => t.id === focusId) ?? null);
  const others = $derived(snap.tiles.filter((t) => t.id !== focused?.id));
  const cols = $derived(Math.ceil(Math.sqrt(snap.tiles.length)));

  function audioFor(id: string) {
    let a = client.volumes.get(id);
    if (!a) client.volumes.set(id, (a = { volume: 1, muted: false }));
    return a;
  }

  function leave() {
    if (isHost && snap.status === "connected" && client.peers.size > 1) {
      if (!confirm("Você é o anfitrião. Sair vai encerrar a sala para todos. Continuar?")) return;
    }
    client.leave();
    if (isHost) invoke("close_room");
    onexit();
  }

  async function startShare(pick: { source: Source; quality: Quality; audio: boolean }) {
    notice = null;
    try {
      const capture = await startNativeCapture(
        pick.source.id,
        pick.quality.height,
        pick.quality.fps,
        pick.audio,
      );
      const title = pick.source.kind === "screen" ? pick.source.title : pick.source.app || pick.source.title;
      client.startShare(capture, pick.quality, title);
    } catch (e) {
      notice = `Não foi possível compartilhar: ${e}`;
    } finally {
      picking = false;
    }
  }

  async function copyCode() {
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
  <aside class="sidebar">
    <div class="room-code">
      <span class="label">Código da sala</span>
      <code title={code}>{code}</code>
      <button class="btn small" onclick={copyCode} title="Copia código + senha num convite só">
        {copied ? "Copiado!" : "Copiar convite"}
      </button>
      <span class="label">Senha</span>
      <code class="password-code">{showPassword ? password : "•".repeat(password.length)}</code>
      <button class="btn small" onclick={() => (showPassword = !showPassword)}>
        {showPassword ? "Ocultar" : "Mostrar"}
      </button>
      <span class="hint">Qualidade da sala: {limits}</span>
      {#if isHost}
        <span class="hint">Você é o anfitrião: a sala fica aberta enquanto seu app estiver aberto.</span>
      {/if}
    </div>

    <div class="section-title">
      <span>Na sala — {snap.peers.length}</span>
      {#if snap.liveOthers > 0}
        <button class="link-btn" onclick={() => client.watchAll()}>Assistir todos</button>
      {/if}
    </div>
    <ul class="peers">
      {#each snap.peers as p (p.id)}
        <li class:active={p.watching}>
          <span class="avatar">{p.name.slice(0, 1).toUpperCase()}</span>
          <div class="peer-info">
            <span class="peer-name">
              {p.name}{#if p.isSelf}<span class="muted"> (você)</span>{/if}
            </span>
            {#if p.sharing && p.label}
              <span class="peer-label" title={p.label}>{p.label}</span>
            {/if}
          </div>
          {#if p.sharing}
            <span class="live">AO VIVO</span>
            <button
              class="btn small"
              class:primary={!p.watching}
              onclick={() => toggleWatch(p.id, p.watching)}
            >
              {p.watching ? "Parar" : p.isSelf ? "Prévia" : "Assistir"}
            </button>
          {/if}
        </li>
      {/each}
    </ul>

    <div class="sidebar-footer">
      {#if snap.sharing}
        <button class="btn big danger" onclick={() => client.stopShare()}>Parar transmissão</button>
      {:else}
        <button
          class="btn big primary"
          disabled={snap.status !== "connected"}
          onclick={() => (picking = true)}>Compartilhar tela</button
        >
      {/if}
      <button class="btn" onclick={leave}>Sair da sala</button>
    </div>
  </aside>

  <main class="stage">
    {#if notice}
      <div class="notice">
        {notice}
        <button class="icon-btn" onclick={() => (notice = null)}>✕</button>
      </div>
    {/if}

    {#if snap.tiles.length === 0}
      <div class="empty">
        <div class="empty-icon">🖥️</div>
        <h2>Nenhuma transmissão selecionada</h2>
        <p class="muted">
          Clique em <b>Assistir</b> em quem está <span class="live">AO VIVO</span> na lista ao lado.
          Você pode assistir várias ao mesmo tempo e clicar em uma para focar nela.
        </p>
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
            ▦ Ver todas
          </button>
        </div>
        {#if others.length > 0}
          <div class="focus-strip">
            {#each others as t (t.id)}
              <Tile
                name={t.name}
                label={t.label}
                view={t.view}
                isSelf={t.isSelf}
                compact
                audio={audioFor(t.id)}
                onselect={() => (focusId = t.id)}
                onclose={() => toggleWatch(t.id, true)}
                onretry={() => client.retry(t.id)}
              />
            {/each}
          </div>
        {/if}
      </div>
    {:else}
      <div class="grid" style:grid-template-columns="repeat({cols}, 1fr)">
        {#each snap.tiles as t (t.id)}
          <Tile
            name={t.name}
            label={t.label}
            view={t.view}
            isSelf={t.isSelf}
            audio={audioFor(t.id)}
            onselect={() => (focusId = t.id)}
            onclose={() => toggleWatch(t.id, true)}
            onretry={() => client.retry(t.id)}
          />
        {/each}
      </div>
    {/if}
  </main>

  {#if picking}
    <SourcePicker settings={client.settings} onpick={startShare} oncancel={() => (picking = false)} />
  {/if}

  {#if snap.status === "closed"}
    <div class="modal-backdrop">
      <div class="card">
        <h2>A sala foi encerrada</h2>
        <p class="muted">O anfitrião saiu ou a conexão caiu.</p>
        <button
          class="btn primary"
          onclick={() => {
            if (isHost) invoke("close_room");
            onexit();
          }}>Voltar ao início</button
        >
      </div>
    </div>
  {/if}
</div>
