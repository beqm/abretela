<script lang="ts" generics="T extends string | number">
  type Props = {
    options: readonly { value: T; label: string }[];
    value: T;
    onchange: (v: T) => void;
  };
  let { options, value, onchange }: Props = $props();
</script>

<div class="segmented">
  {#each options as opt (opt.value)}
    <button type="button" class:selected={opt.value === value} onclick={() => onchange(opt.value)}
      >{opt.label}</button
    >
  {/each}
</div>

<style>
  .segmented {
    display: inline-flex;
    background: var(--surface-2);
    border-radius: var(--r);
    padding: 3px;
    gap: 2px;
  }

  .segmented button {
    flex: 1;
    background: none;
    border: none;
    border-radius: 7px;
    padding: 4px 12px;
    font-size: 13px;
    font-weight: 700;
    color: var(--muted);
    cursor: pointer;
    white-space: nowrap;
    transition:
      background 0.12s,
      color 0.12s;
  }

  .segmented button:hover {
    color: var(--text);
  }

  .segmented button.selected {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-sm);
  }
</style>
