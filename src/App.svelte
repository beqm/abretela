<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrent, onOpenUrl } from "@tauri-apps/plugin-deep-link";
  import Home from "./Home.svelte";
  import Room from "./Room.svelte";
  import Updater from "./Updater.svelte";
  import { parseInvite, roomLabel, type RoomClient } from "./lib/room";
  import { log } from "./lib/log";
  import { prefs, t } from "./lib/prefs.svelte";

  type ActiveRoom = { client: RoomClient; room: string; password: string; isHost: boolean };

  let ready = $state(false);
  let room = $state<ActiveRoom | null>(null);
  /** Link de convite (abretela://...) que chegou e ainda não foi usado. */
  let link = $state<string | null>(null);

  if (import.meta.env.DEV && location.search.includes("mock")) {
    import("./dev/mockRoom").then(({ mockRoom }) => {
      room = {
        client: mockRoom(location.search.includes("empty")),
        room: "https://ancient-kevin-hollow-tribe.trycloudflare.com",
        password: "482913",
        isHost: true,
      };
    });
  }

  function openLink(url: string | undefined) {
    if (!url) return;
    const invite = parseInvite(url);
    if (!invite) {
      log.warn("room", "Got a link that isn't a valid room invite");
      return;
    }
    log.info("room", `Opened an invite link to ${roomLabel(invite.room)}`);
    link = url;
  }

  // links abretela:// que abriram o app ou chegaram com ele já aberto
  onMount(() => {
    getCurrent()
      .then((urls) => openLink(urls?.[0]))
      .catch(() => {});
    const unlisten = onOpenUrl((urls) => openLink(urls[0])).catch(() => null);
    return () => {
      unlisten.then((stop) => stop?.());
    };
  });

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
    roomUrl={room.room}
    password={room.password}
    isHost={room.isHost}
    {link}
    onlinkdismiss={() => (link = null)}
    onexit={() => (room = null)}
  />
{:else}
  <Home onenter={(r) => (room = r)} {link} onlinkused={() => (link = null)} />
{/if}
