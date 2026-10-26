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
