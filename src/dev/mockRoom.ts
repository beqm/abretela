// Só em desenvolvimento: uma sala falsa (?mock) para ver a interface no
// navegador sem Tauri nem servidor. Vídeos são canvas animados.

import { snapshot, type PeerInfo, type RemoteView, type RoomClient } from "../lib/room";
import { DEFAULT_SETTINGS } from "../lib/quality";

function fakeStream(hue: number, text: string): MediaStream {
  const canvas = document.createElement("canvas");
  canvas.width = 1280;
  canvas.height = 720;
  const ctx = canvas.getContext("2d")!;
  let t = 0;
  setInterval(() => {
    t++;
    const g = ctx.createLinearGradient(0, 0, 1280, 720);
    g.addColorStop(0, `hsl(${hue} 45% 22%)`);
    g.addColorStop(1, `hsl(${hue + 40} 50% 12%)`);
    ctx.fillStyle = g;
    ctx.fillRect(0, 0, 1280, 720);
    ctx.fillStyle = `hsl(${hue} 70% 60%)`;
    ctx.beginPath();
    ctx.arc(640 + Math.sin(t / 20) * 300, 360 + Math.cos(t / 25) * 160, 60, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = "rgba(255,255,255,.8)";
    ctx.font = "bold 48px sans-serif";
    ctx.fillText(text, 60, 100);
  }, 33);
  return canvas.captureStream(30);
}

export function mockRoom(empty = false): RoomClient {
  const listeners = new Set<() => void>();
  const peers = new Map<string, PeerInfo>(
    (
      [
        ["1", "Breno", true, "1080p 60fps · Valorant"],
        ["2", "Ana", true, "720p 60fps · Minecraft"],
        ["3", "Lucas", true, "1080p 30fps · Tela 1"],
        ["4", "Júlia", false, null],
        ["5", "Pedro", true, "720p 30fps · Hollow Knight"],
      ] as const
    ).map(([id, name, sharing, label]) => [id, { id, name, sharing, label }]),
  );
  const views = new Map<string, RemoteView>([
    ["2", { stream: fakeStream(200, "Minecraft"), state: "connected" }],
    ["3", { stream: fakeStream(280, "Tela 1"), state: "connected" }],
    ["5", { stream: null, state: "connecting" }],
  ]);
  const emit = () => listeners.forEach((f) => f());
  const client = {
    selfId: "1",
    peers,
    views,
    watching: new Set(empty ? [] : ["1", "2", "3", "5"]),
    localStream: (empty ? null : fakeStream(20, "Valorant")) as MediaStream | null,
    status: "connected",
    settings: DEFAULT_SETTINGS,
    volumes: new Map(),
    onPeerEvent: null,
    subscribe: (fn: () => void) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    watch(id: string) {
      client.watching.add(id);
      emit();
    },
    unwatch(id: string) {
      client.watching.delete(id);
      emit();
    },
    watchAll() {},
    retry() {},
    setViewSize() {},
    setPip() {},
    thumbs: new Map<string, { image: string | null; at: number }>(),
    async requestThumb(id: string) {
      const stream = views.get(id)?.stream;
      client.thumbs.set(id, { image: stream ? await snapshot(stream) : null, at: Date.now() });
      emit();
    },
    isThumbLoading: () => false,
    releaseCapture() {},
    switchShare() {},
    leave() {},
    stopShare() {
      client.localStream = null;
      peers.get("1")!.sharing = false;
      client.watching.delete("1");
      emit();
    },
    startShare() {},
  };
  return client as unknown as RoomClient;
}
