<script lang="ts">
  let { name, size = 32 }: { name: string; size?: number } = $props();

  const COLORS = ["#FF7A2F", "#22C5B0", "#7C5CFF", "#FFB020", "#FF4D8D", "#3BA3FF"];

  const color = $derived.by(() => {
    let h = 0;
    for (const ch of name) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
    return COLORS[h % COLORS.length];
  });
</script>

<span
  class="avatar"
  title={name}
  style:background={color}
  style:width="{size}px"
  style:height="{size}px"
  style:font-size="{size * 0.45}px">{name.slice(0, 1).toUpperCase()}</span
>
