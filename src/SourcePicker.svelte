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
  import { t } from "./lib/prefs.svelte";
  import Segmented from "./Segmented.svelte";
  import Icon from "./Icon.svelte";

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
      <h2>{t("shareTitle")}</h2>
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
        {starting ? t("starting") : t("startStreaming")}
      </button>
    </footer>
  </div>
</div>
