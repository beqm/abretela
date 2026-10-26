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
