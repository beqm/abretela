<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import Home from "./Home.svelte";
  import Room from "./Room.svelte";
  import Updater from "./Updater.svelte";
  import type { RoomClient } from "./lib/room";
  import { prefs, t } from "./lib/prefs.svelte";

  type ActiveRoom = { client: RoomClient; code: string; password: string; isHost: boolean };

  let ready = $state(false);
  let room = $state<ActiveRoom | null>(null);

  // mantém a bandeja (textos e comportamento do X) em sincronia com as preferências
  $effect(() => {
    invoke("set_tray_texts", {
      open: t("trayOpen"),
      quit: t("trayQuit"),
      tooltip: "AbreTela",
      liveTooltip: t("trayLive"),
    }).catch(() => {});
  });
  $effect(() => {
    invoke("set_close_to_tray", { enabled: prefs.closeToTray }).catch(() => {});
  });
</script>

{#if !ready}
  <Updater ondone={() => (ready = true)} />
{:else if room}
  <Room
    client={room.client}
    code={room.code}
    password={room.password}
    isHost={room.isHost}
    onexit={() => (room = null)}
  />
{:else}
  <Home onenter={(r) => (room = r)} />
{/if}
