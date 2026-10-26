<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import type { RemoteView } from "./lib/room";

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
<div
  class="tile"
  class:compact
  class:focused
  bind:this={tile}
  onclick={onselect}
  ondblclick={toggleFullscreen}
>
  <video
    use:srcObject={view.stream}
    autoplay
    playsinline
    muted={isSelf || muted}
    bind:volume
  ></video>

  {#if view.state !== "connected" && !isSelf}
    <div class="tile-overlay">
      {#if view.state === "connecting"}
        <span class="spinner"></span>
        {#if !compact}Conectando...{/if}
      {:else}
        <span>Falha na conexão{compact ? "" : ` com ${name}`}.</span>
        <button class="btn small" onclick={stop(onretry)}>Tentar de novo</button>
      {/if}
    </div>
  {/if}

  <div class="tile-tag">
    <span class="live">AO VIVO</span>
    <span class="tile-name">{name}{isSelf ? " (prévia)" : ""}</span>
  </div>

  {#if !compact}
    <div class="tile-bar">
      <span class="tile-label muted">{label ?? ""}</span>
      <div class="tile-controls">
        {#if !isSelf}
          <button
            class="icon-btn"
            title={muted ? "Ativar som" : "Silenciar"}
            onclick={stop(() => (muted = !muted))}>{muted || volume === 0 ? "🔇" : "🔊"}</button
          >
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
        <button class="icon-btn" title="Tela cheia" onclick={toggleFullscreen}>⛶</button>
        <button class="icon-btn" title="Parar de assistir" onclick={stop(onclose)}>✕</button>
      </div>
    </div>
  {/if}
</div>
