<script lang="ts">
  import { onMount } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import { openUrl } from "@tauri-apps/plugin-opener";

  const REPO = "https://github.com/beqm/abretela";
  const AUTHOR = "https://github.com/beqm";

  let version = $state("");
  onMount(async () => {
    version = await getVersion();
  });

  const open = (url: string) => (e: MouseEvent) => {
    e.preventDefault();
    openUrl(url);
  };
</script>

<div class="credits">
  <span>AbreTela{version ? ` v${version}` : ""}</span>
  <span>·</span>
  <span>desenvolvido por <a href={AUTHOR} onclick={open(AUTHOR)}>@beqm</a></span>
  <span>·</span>
  <a href={REPO} onclick={open(REPO)}>GitHub</a>
</div>
