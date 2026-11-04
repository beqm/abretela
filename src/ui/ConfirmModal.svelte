<script lang="ts">
  import { onMount } from "svelte";

  type Props = {
    title: string;
    message: string;
    confirmLabel: string;
    cancelLabel: string;
    danger?: boolean;
    onconfirm: () => void;
    oncancel: () => void;
  };
  let { title, message, confirmLabel, cancelLabel, danger = false, onconfirm, oncancel }: Props = $props();

  let confirmBtn: HTMLButtonElement;

  onMount(() => {
    confirmBtn.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") oncancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="modal-backdrop" onclick={(e) => e.target === e.currentTarget && oncancel()}>
  <div class="modal confirm-modal" role="alertdialog" aria-labelledby="confirm-title">
    <img class="confirm-mascot" src="/logo.svg" alt="" />
    <h2 id="confirm-title">{title}</h2>
    <p class="muted">{message}</p>
    <div class="confirm-actions">
      <button class="btn" onclick={oncancel}>{cancelLabel}</button>
      <button class="btn" class:danger class:primary={!danger} bind:this={confirmBtn} onclick={onconfirm}>
        {confirmLabel}
      </button>
    </div>
  </div>
</div>

<style>
  .confirm-modal {
    width: min(380px, 100%);
    padding: 24px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    text-align: center;
  }

  .confirm-mascot {
    width: 72px;
    height: 72px;
    margin-bottom: 6px;
    animation: wobble 2.4s ease-in-out infinite;
  }

  @keyframes wobble {
    0%,
    100% {
      transform: rotate(-5deg);
    }
    50% {
      transform: rotate(5deg);
    }
  }

  .confirm-actions {
    display: flex;
    gap: 8px;
    margin-top: 14px;
    width: 100%;
  }

  .confirm-actions .btn {
    flex: 1;
  }
</style>
