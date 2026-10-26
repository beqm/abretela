<script lang="ts">
  type Props = {
    options: readonly number[];
    value: number;
    format?: (v: number) => string;
    disabled?: (v: number) => boolean;
    onchange: (v: number) => void;
  };
  let { options, value, format = String, disabled = () => false, onchange }: Props = $props();
</script>

<div class="segmented">
  {#each options as opt (opt)}
    <button
      type="button"
      class:selected={opt === value}
      disabled={disabled(opt)}
      onclick={() => onchange(opt)}>{format(opt)}</button
    >
  {/each}
</div>
