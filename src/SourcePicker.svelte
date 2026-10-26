<script lang="ts">
  import { onMount } from "svelte";
  import { getThumbnail, listSources, type Source } from "./lib/capture";
  import {
    allowedFps,
    allowedHeights,
    clampQuality,
    loadJson,
    saveJson,
    type Quality,
    type RoomSettings,
  } from "./lib/quality";
  import Segmented from "./Segmented.svelte";

  type Props = {
    settings: RoomSettings;
    onpick: (pick: { source: Source; quality: Quality; audio: boolean }) => void;
    oncancel: () => void;
  };
  let { settings, onpick, oncancel }: Props = $props();

  const PREFS_KEY = "abretela.sharePrefs";
  const prefs = loadJson(PREFS_KEY, { height: 720, fps: 30, audio: true });

  let tab = $state<"window" | "screen">("window");
  let sources = $state<Source[]>([]);
  let thumbs = $state<Record<string, string | null>>({});
  let loading = $state(true);
  let selected = $state<string | null>(null);
  // svelte-ignore state_referenced_locally
  let quality = $state<Quality>(clampQuality(prefs, settings));
  let audio = $state(prefs.audio);
  let starting = $state(false);

  const visible = $derived(sources.filter((s) => s.kind === tab));
  const heights = $derived(allowedHeights(settings));
  const fpsOptions = $derived(allowedFps(settings));
  const selectedSource = $derived(sources.find((s) => s.id === selected));

  async function refresh() {
    loading = true;
    sources = await listSources();
    thumbs = {};
    loading = false;
    if (selected && !sources.some((s) => s.id === selected)) selected = null;
    // miniaturas em paralelo, 4 por vez
    const queue = [...sources];
    const worker = async () => {
      for (let s = queue.shift(); s; s = queue.shift()) {
        thumbs[s.id] = await getThumbnail(s.id);
      }
    };
    await Promise.all([worker(), worker(), worker(), worker()]);
  }

  onMount(() => {
    refresh();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && oncancel();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  function go() {
    if (!selectedSource || starting) return;
    starting = true;
    saveJson(PREFS_KEY, { ...quality, audio });
    onpick({ source: selectedSource, quality: { ...quality }, audio });
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="modal-backdrop" onclick={(e) => e.target === e.currentTarget && oncancel()}>
  <div class="picker">
    <header>
      <h2>Compartilhar</h2>
      <div class="tabs">
        <button class:active={tab === "window"} onclick={() => (tab = "window")}>Aplicativos</button>
        <button class:active={tab === "screen"} onclick={() => (tab = "screen")}>Telas</button>
      </div>
      <button class="icon-btn" title="Atualizar" onclick={refresh}>⟳</button>
      <button class="icon-btn" title="Fechar" onclick={oncancel}>✕</button>
    </header>

    <div class="sources">
      {#if loading}
        <div class="picker-empty"><span class="spinner"></span> Procurando...</div>
      {:else if visible.length === 0}
        <div class="picker-empty muted">Nada encontrado.</div>
      {:else}
        {#each visible as s (s.id)}
          <button
            class="source"
            class:selected={selected === s.id}
            onclick={() => (selected = s.id)}
            ondblclick={() => {
              selected = s.id;
              go();
            }}
          >
            <div class="thumb">
              {#if thumbs[s.id]}
                <img src={thumbs[s.id]} alt="" />
              {:else if s.id in thumbs}
                <span class="muted">Sem prévia</span>
              {:else}
                <span class="spinner"></span>
              {/if}
            </div>
            <div class="source-title" title={s.title}>{s.title}</div>
            <div class="source-app muted">{s.app}</div>
          </button>
        {/each}
      {/if}
    </div>

    <footer>
      <div class="opt">
        <span class="settings-label">Resolução</span>
        <Segmented
          options={heights}
          value={quality.height}
          format={(v) => `${v}p`}
          onchange={(v) => (quality.height = v)}
        />
      </div>
      <div class="opt">
        <span class="settings-label">FPS</span>
        <Segmented options={fpsOptions} value={quality.fps} onchange={(v) => (quality.fps = v)} />
      </div>
      <label class="check">
        <input type="checkbox" bind:checked={audio} />
        {tab === "window" ? "Áudio do aplicativo" : "Áudio do sistema"}
      </label>
      <div class="spacer"></div>
      <button class="btn" onclick={oncancel}>Cancelar</button>
      <button class="btn primary" disabled={!selectedSource || starting} onclick={go}>
        {starting ? "Iniciando..." : "Transmitir"}
      </button>
    </footer>
  </div>
</div>
