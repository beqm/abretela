<script lang="ts">
  type Props = {
    label: string;
    hint?: string;
    checked: boolean;
    disabled?: boolean;
    onchange: (v: boolean) => void;
  };
  let { label, hint, checked, disabled = false, onchange }: Props = $props();
</script>

<label class="toggle-row" class:disabled>
  <span class="toggle-text">
    <span class="toggle-label">{label}</span>
    {#if hint}<span class="hint">{hint}</span>{/if}
  </span>
  <input
    type="checkbox"
    class="toggle"
    {checked}
    {disabled}
    onchange={(e) => onchange(e.currentTarget.checked)}
  />
</label>

<style>
  .toggle-row {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 10px 12px;
    cursor: pointer;
    transition: background 0.12s;
  }

  .toggle-row:hover {
    background: var(--surface-2);
  }

  .toggle-row.disabled {
    opacity: 0.5;
    cursor: default;
  }

  .toggle-text {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .toggle-label {
    font-weight: 700;
  }

  .toggle {
    appearance: none;
    flex-shrink: 0;
    width: 36px;
    height: 20px;
    padding: 0;
    border-radius: 999px;
    border: none;
    background: var(--line-strong);
    position: relative;
    cursor: pointer;
    transition: background 0.15s;
  }

  .toggle:focus {
    box-shadow: none;
  }

  .toggle::after {
    content: "";
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.25);
    transition: transform 0.15s;
  }

  .toggle:checked {
    background: var(--primary);
  }

  .toggle:checked::after {
    transform: translateX(16px);
  }

  .toggle:disabled {
    cursor: default;
  }
</style>
