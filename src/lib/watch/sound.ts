// O som das telas assistidas pela recepção nativa (`decode.ts`).
//
// Duas coisas do WebKitGTK decidem como isto é feito:
//
// - Ele só deixa um `AudioContext` tocar se for criado ou retomado durante um
//   gesto da pessoa (clique, tecla). A recepção monta o tocador quando a
//   oferta de quem transmite chega pela rede, bem depois do clique em
//   Assistir: um contexto criado ali nasce suspenso. Por isso há um contexto
//   só, para todas as telas assistidas, e cada gesto na janela o acorda. O
//   clique em Assistir já é um deles.
// - O som de uma faixa tirada do contexto (`MediaStreamAudioDestinationNode`)
//   não toca num elemento de vídeo: o GStreamer descarta todas as amostras
//   como atrasadas (medido no 2.52). Por isso o tocador vai direto para a
//   saída do contexto, e o volume de cada tela é um ganho aqui, não o do
//   elemento de vídeo.

let context: AudioContext | null = null;
let worklet: Promise<void> | null = null;
let armed = false;

function shared(): AudioContext {
  context ??= new AudioContext({ sampleRate: 48000, latencyHint: "interactive" });
  return context;
}

function wake() {
  const ctx = shared();
  if (ctx.state === "suspended") ctx.resume().catch(() => {});
}

/**
 * Passa a acordar o som a cada gesto na janela. Chamar assim que se sabe que
 * as telas vão ser assistidas pela recepção nativa (ao entrar na sala).
 */
export function armSound() {
  if (armed || typeof window === "undefined") return;
  armed = true;
  for (const gesture of ["pointerdown", "pointerup", "keydown"]) {
    window.addEventListener(gesture, wake, { capture: true, passive: true });
  }
}

export type Player = {
  /** Toca amostras estéreo intercaladas (f32, 48 kHz). */
  play(samples: Float32Array): void;
  /** O volume desta tela, de 0 (mudo) a 1. */
  setVolume(volume: number): void;
  /** O sistema está deixando o som sair? */
  audible(): boolean;
  close(): void;
};

/** Um tocador para uma tela assistida (o mesmo da captura nativa: `/audio-worklet.js`). */
export async function openPlayer(): Promise<Player> {
  const ctx = shared();
  worklet ??= ctx.audioWorklet.addModule("/audio-worklet.js");
  try {
    await worklet;
  } catch (e) {
    worklet = null; // a próxima tela tenta de novo
    throw e;
  }
  const node = new AudioWorkletNode(ctx, "pcm-player", { numberOfInputs: 0, outputChannelCount: [2] });
  const gain = ctx.createGain();
  node.connect(gain);
  gain.connect(ctx.destination);
  // sem esperar: se o sistema segurar o som, o vídeo não pode ficar preso junto
  ctx.resume().catch(() => {});
  return {
    play: (samples) => node.port.postMessage(samples, [samples.buffer]),
    setVolume: (volume) => {
      gain.gain.value = Math.min(1, Math.max(0, volume));
    },
    audible: () => ctx.state === "running",
    close() {
      // o contexto fica: é de todas as telas
      node.disconnect();
      gain.disconnect();
    },
  };
}
