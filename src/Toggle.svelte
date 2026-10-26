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
