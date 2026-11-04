<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { findUpdate, installUpdate } from "../lib/update";
  import { describeError, log } from "../lib/log";

  /** Baixando uma atualização: mostra a barra (fora isso, só o mascote). */
  let downloading = $state(false);
  let progress = $state<number | null>(null);

  onMount(async () => {
    if (!import.meta.env.DEV) {
      try {
        const update = await findUpdate();
        if (update) {
          const v = update.version;
          log.info("update", `Updating to version ${v}`);
          invoke("splash_status", { updating: true }).catch(() => {});
          downloading = true;
          try {
            // instala e reinicia o app; daqui não volta
            await installUpdate(update, (phase, pct) => {
              progress = phase === "install" ? null : pct;
            });
            return;
          } catch (e) {
            log.error("update", `The update failed, opening the current version: ${describeError(e)}`);
            invoke("splash_status", { updating: false }).catch(() => {});
            downloading = false;
          }
        }
      } catch (e) {
        // sem internet, GitHub fora do ar etc.: abre o app normalmente
        log.warn("update", `Could not check for updates: ${describeError(e)}`);
      }
    }
    invoke("splash_done").catch(() => {});
  });
</script>

<div class="splash" data-tauri-drag-region>
  <img class="mascot" src="/logo.svg" alt="" data-tauri-drag-region />
  <span class="shadow"></span>
  {#if downloading}
    <div class="bar" class:indeterminate={progress === null}>
      <div style:width={progress === null ? null : `${progress}%`}></div>
    </div>
  {/if}
</div>

<style>
  :global(html),
  :global(body) {
    margin: 0;
    height: 100%;
    background: transparent;
    overflow: hidden;
    user-select: none;
  }

  :global(#root) {
    height: 100%;
  }

  .splash {
    box-sizing: border-box;
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 24px;
    background: #17120f;
    border: 1px solid #31271f;
    border-radius: 18px;
    position: relative;
    cursor: default;
  }

  .mascot {
    width: 120px;
    height: 120px;
    animation: hop 1.1s cubic-bezier(0.5, 0, 0.5, 1) infinite;
    transform-origin: 50% 100%;
    -webkit-user-drag: none;
  }

  /* pulo: agacha, sobe, cai e amassa um pouco ao tocar o chão */
  @keyframes hop {
    0%,
    100% {
      transform: translateY(0) scale(1.08, 0.92);
    }
    15% {
      transform: translateY(0) scale(0.96, 1.04);
    }
    50% {
      transform: translateY(-34px) scale(1, 1);
    }
    85% {
      transform: translateY(0) scale(0.98, 1.02);
    }
  }

  .shadow {
    width: 70px;
    height: 10px;
    margin-top: 4px;
    border-radius: 50%;
    background: rgba(0, 0, 0, 0.45);
    animation: shadow 1.1s cubic-bezier(0.5, 0, 0.5, 1) infinite;
  }

  @keyframes shadow {
    0%,
    100% {
      transform: scale(1);
      opacity: 1;
    }
    50% {
      transform: scale(0.6);
      opacity: 0.5;
    }
  }

  .bar {
    width: 160px;
    height: 4px;
    position: absolute;
    bottom: 26px;
    border-radius: 999px;
    background: #2a221d;
    overflow: hidden;
  }

  .bar div {
    height: 100%;
    border-radius: 999px;
    background: #f26a1f;
    transition: width 0.2s;
  }

  .bar.indeterminate div {
    width: 40%;
    animation: slide 1.1s ease-in-out infinite;
  }

  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(250%);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .mascot,
    .shadow,
    .bar.indeterminate div {
      animation: none;
    }
  }
</style>
