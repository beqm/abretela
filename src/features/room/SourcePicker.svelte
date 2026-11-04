<script lang="ts">
  import { onMount } from "svelte";
  import { getThumbnail, listSources, type Source } from "../../lib/capture/native";
  import {
    allowedFps,
    allowedHeights,
    clampQuality,
    loadJson,
    saveJson,
    type Quality,
    type RoomSettings,
  } from "../../lib/quality";
  import { t } from "../../lib/prefs.svelte";
  import Segmented from "../../ui/Segmented.svelte";
  import Icon from "../../ui/Icon.svelte";

  type Props = {
    settings: RoomSettings;
    onpick: (pick: { source: Source; quality: Quality; audio: boolean }) => void;
    oncancel: () => void;
    /** Trocando a tela de uma transmissão que já está no ar. */
    switching?: boolean;
  };
  let { settings, onpick, oncancel, switching = false }: Props = $props();

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
  const heights = $derived(allowedHeights(settings).map((h) => ({ value: h as number, label: `${h}p` })));
  const fpsOptions = $derived(allowedFps(settings).map((f) => ({ value: f as number, label: String(f) })));
  const tabs = $derived([
    { value: "window" as const, label: t("apps") },
    { value: "screen" as const, label: t("screens") },
  ]);
  const selectedSource = $derived(sources.find((s) => s.id === selected));

  const titleOf = (s: Source) => (s.kind === "screen" ? t("screenN", { n: s.title }) : s.title);

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
  <div class="modal picker">
    <header class="modal-head">
      <h2>{switching ? t("switchScreenTitle") : t("shareTitle")}</h2>
      <Segmented options={tabs} value={tab} onchange={(v) => (tab = v)} />
      <div class="spacer"></div>
      <button class="icon-btn" title={t("refresh")} onclick={refresh}><Icon name="refresh" /></button>
      <button class="icon-btn" title={t("close")} onclick={oncancel}><Icon name="x" /></button>
    </header>

    <div class="sources">
      {#if loading}
        <div class="picker-empty"><span class="spinner"></span> {t("searching")}</div>
      {:else if visible.length === 0}
        <div class="picker-empty muted">{t("nothingFound")}</div>
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
                <span class="muted"><Icon name={s.kind === "screen" ? "monitor" : "tv"} size={28} /></span>
              {:else}
                <span class="spinner"></span>
              {/if}
            </div>
            <div class="source-title" title={titleOf(s)}>{titleOf(s)}</div>
            <div class="source-app muted">{s.app}</div>
          </button>
        {/each}
      {/if}
    </div>

    <footer class="picker-foot">
      <div class="opt">
        <span class="opt-label">{t("resolution")}</span>
        <Segmented options={heights} value={quality.height} onchange={(v) => (quality.height = v)} />
      </div>
      <div class="opt">
        <span class="opt-label">{t("fps")}</span>
        <Segmented options={fpsOptions} value={quality.fps} onchange={(v) => (quality.fps = v)} />
      </div>
      <label class="check">
        <input type="checkbox" bind:checked={audio} />
        {tab === "window" ? t("appAudio") : t("systemAudio")}
      </label>
      <div class="spacer"></div>
      <button class="btn" onclick={oncancel}>{t("cancel")}</button>
      <button class="btn primary" disabled={!selectedSource || starting} onclick={go}>
        {starting ? t("starting") : switching ? t("switchTo") : t("startStreaming")}
      </button>
    </footer>
  </div>
</div>

<style>
  .modal-head {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .picker {
    width: min(920px, 100%);
    height: min(660px, 100%);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .picker .modal-head {
    padding: 12px 12px 12px 18px;
    border-bottom: 1px solid var(--line);
  }

  .sources {
    flex: 1;
    overflow-y: auto;
    padding: 16px;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: 12px;
    align-content: start;
  }

  .picker-empty {
    grid-column: 1 / -1;
    display: flex;
    justify-content: center;
    align-items: center;
    gap: 10px;
    padding: 40px;
  }

  .source {
    background: none;
    border: none;
    border-radius: var(--r);
    padding: 6px;
    cursor: pointer;
    text-align: left;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    transition: background 0.12s;
  }

  .source:hover {
    background: var(--surface-2);
  }

  .source.selected {
    background: var(--primary-soft);
  }

  .source.selected .thumb {
    outline-color: var(--primary);
  }

  .thumb {
    aspect-ratio: 16 / 9;
    background: var(--stage);
    border-radius: var(--r-sm);
    display: grid;
    place-items: center;
    overflow: hidden;
    color: #fff;
    margin-bottom: 6px;
    outline: 2px solid transparent;
    outline-offset: 1px;
    transition: outline-color 0.12s;
  }

  .thumb .spinner {
    border-color: rgba(255, 255, 255, 0.15);
    border-top-color: rgba(255, 255, 255, 0.7);
  }

  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  .source-title {
    font-weight: 700;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: 0 2px;
  }

  .source-app {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: 0 2px;
  }

  .picker-foot {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 16px;
    padding: 12px 16px;
    border-top: 1px solid var(--line);
  }

  .picker-foot .opt {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .opt-label {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    font-weight: 700;
    color: var(--muted);
    white-space: nowrap;
  }
</style>
