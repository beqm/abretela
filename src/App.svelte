<script lang="ts">
  import Home from "./Home.svelte";
  import Room from "./Room.svelte";
  import type { RoomClient } from "./lib/room";

  type ActiveRoom = { client: RoomClient; code: string; password: string; isHost: boolean };

  let room = $state<ActiveRoom | null>(null);
</script>

{#if room}
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
