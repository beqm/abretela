<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import type { RemoteView } from "../../lib/transport/types";
  import { t } from "../../lib/prefs.svelte";
  import Icon from "../../ui/Icon.svelte";

  type Props = {
    name: string;
    label?: string | null;
    view: RemoteView;
    isSelf: boolean;
    compact?: boolean;
    focused?: boolean;
    audio: { volume: number; muted: boolean };
    onselect: () => void;
    onclose: () => void;
    onretry: () => void;
    /** Avisado quando o tile muda de tamanho (para pedir a qualidade certa). */
    onsize?: (width: number, height: number) => void;
    /** Abre/fecha esta tela numa janela flutuante (sem isso, não mostra o botão). */
    onpopout?: () => void;
    /** Esta tela está aberta na janela flutuante. */
    popped?: boolean;
  };
  let {
    name,
    label,
    view,
    isSelf,
    compact = false,
    focused = false,
    audio,
    onselect,
    onclose,
    onretry,
    onsize,
    onpopout,
    popped = false,
  }: Props = $props();

  let tile: HTMLDivElement;
  // svelte-ignore state_referenced_locally
  let volume = $state(audio.volume);
  // svelte-ignore state_referenced_locally
  let muted = $state(audio.muted);

  $effect(() => {
    audio.volume = volume;
    audio.muted = muted;
  });

  function srcObject(node: HTMLVideoElement, stream: MediaStream | null) {
    node.srcObject = stream;
    return {
      update(next: MediaStream | null) {
        if (node.srcObject !== next) node.srcObject = next;
      },
    };
  }

  onMount(() => {
    const observer = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect;
      if (width && height) onsize?.(width, height);
    });
    observer.observe(tile);
    return () => observer.disconnect();
  });

  onMount(() => {
    const onChange = () => {
      if (!document.fullscreenElement) getCurrentWindow().setFullscreen(false);
    };
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  });

  async function toggleFullscreen(e?: Event) {
    e?.stopPropagation();
    if (document.fullscreenElement) {
      await document.exitFullscreen();
    } else {
      await tile.requestFullscreen();
      await getCurrentWindow().setFullscreen(true);
    }
  }

  const stop = (fn: () => void) => (e: Event) => {
    e.stopPropagation();
    fn();
  };
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="tile" class:compact class:focused bind:this={tile} onclick={onselect} ondblclick={toggleFullscreen}>
  <video use:srcObject={view.stream} autoplay playsinline muted={isSelf || muted} bind:volume></video>

  {#if popped}
    <div class="tile-overlay popped">
      <Icon name="pip" size={compact ? 20 : 28} />
      {#if !compact}
        <span>{t("poppedOut")}</span>
        <button class="btn small" onclick={stop(() => onpopout?.())}>{t("bringBack")}</button>
      {/if}
    </div>
  {:else if view.state !== "connected" && !isSelf}
    <div class="tile-overlay">
      {#if view.state === "connecting"}
        <span class="spinner"></span>
        {#if !compact}{t("connectingStream")}{/if}
      {:else}
        <span>{compact ? t("connectionFailed") : t("connectionFailedWith", { name })}</span>
        <button class="btn small" onclick={stop(onretry)}>{t("tryAgain")}</button>
      {/if}
    </div>
  {/if}

  <div class="tile-tag">
    <span class="live-dot"></span>
    <span class="tile-name">{name}{isSelf ? ` ${t("previewSuffix")}` : ""}</span>
  </div>

  {#if !compact}
    <div class="tile-bar">
      <span class="tile-label">{label ?? ""}</span>
      <div class="tile-controls">
        {#if !isSelf}
          <button class="icon-btn" title={muted ? t("unmute") : t("mute")} onclick={stop(() => (muted = !muted))}>
            <Icon name={muted || volume === 0 ? "mute" : "volume"} />
          </button>
          <input
            type="range"
            min="0"
            max="1"
            step="0.01"
            value={muted ? 0 : volume}
            onclick={(e) => e.stopPropagation()}
            oninput={(e) => {
              volume = Number(e.currentTarget.value);
              muted = false;
            }}
          />
        {/if}
        {#if onpopout && view.stream}
          <button class="icon-btn" class:active={popped} title={popped ? t("bringBack") : t("popOut")} onclick={stop(onpopout)}>
            <Icon name="pip" />
          </button>
        {/if}
        <button class="icon-btn" title={t("fullscreen")} onclick={toggleFullscreen}><Icon name="maximize" /></button>
        <button class="icon-btn" title={t("stopWatching")} onclick={stop(onclose)}><Icon name="x" /></button>
      </div>
    </div>
  {/if}
</div>

<style>
  .tile {
    position: relative;
    background: var(--stage);
    border-radius: var(--r);
    overflow: hidden;
    min-height: 0;
    cursor: pointer;
    outline: 2px solid transparent;
    outline-offset: -2px;
    transition: outline-color 0.12s;
  }

  .tile:hover {
    outline-color: color-mix(in srgb, var(--primary) 70%, transparent);
  }

  .tile.focused {
    cursor: default;
    outline-color: transparent;
  }

  .tile:fullscreen {
    border-radius: 0;
    outline: none;
  }

  .tile video {
    width: 100%;
    height: 100%;
    object-fit: contain;
    display: block;
  }

  .tile-overlay {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    background: rgba(0, 0, 0, 0.6);
    color: rgba(255, 255, 255, 0.85);
    font-size: 13px;
    text-align: center;
    padding: 8px;
  }

  .tile-overlay .spinner {
    border-color: rgba(255, 255, 255, 0.2);
    border-top-color: #fff;
  }

  .tile-overlay .btn {
    background: rgba(255, 255, 255, 0.12);
    border-color: transparent;
    color: #fff;
    box-shadow: none;
  }

  .tile-overlay .btn:hover {
    background: rgba(255, 255, 255, 0.2);
  }

  .tile-overlay.popped {
    background: rgba(0, 0, 0, 0.82);
  }

  .tile-bar .icon-btn.active {
    color: var(--primary-text);
  }

  .tile-tag {
    position: absolute;
    top: 8px;
    left: 8px;
    display: flex;
    align-items: center;
    gap: 6px;
    max-width: calc(100% - 16px);
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(8px);
    color: #fff;
    padding: 3px 8px;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 700;
    pointer-events: none;
  }

  .tile-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tile-bar {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 32px 10px 8px 12px;
    background: linear-gradient(transparent, rgba(0, 0, 0, 0.75));
    color: #fff;
    opacity: 0;
    transition: opacity 0.15s;
  }

  .tile:hover .tile-bar {
    opacity: 1;
  }

  .tile-bar .icon-btn {
    color: rgba(255, 255, 255, 0.85);
  }

  .tile-bar .icon-btn:hover {
    background: rgba(255, 255, 255, 0.15);
    color: #fff;
  }

  .tile-label {
    font-size: 12px;
    opacity: 0.8;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tile-controls {
    display: flex;
    align-items: center;
    gap: 2px;
  }

  .tile-controls input[type="range"] {
    width: 84px;
    height: auto;
    padding: 0;
    border: none;
    background: none;
    box-shadow: none;
    accent-color: #fff;
  }

  .tile.compact .tile-tag {
    top: auto;
    bottom: 6px;
    left: 6px;
    font-size: 11px;
    padding: 2px 6px;
  }
</style>
