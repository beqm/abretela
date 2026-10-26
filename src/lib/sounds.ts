// Efeitos sonoros sintetizados (sem arquivos): "plim" fofo de entrada,
// descendo na saída e um arpejo quando alguém abre a tela.

import { prefs } from "./prefs.svelte";

let ctx: AudioContext | null = null;

function play(notes: number[], gap = 0.085, length = 0.16) {
  if (!prefs.sounds) return;
  ctx ??= new AudioContext();
  const now = ctx.currentTime + 0.01;
  notes.forEach((freq, i) => {
    const t0 = now + i * gap;
    const osc = ctx!.createOscillator();
    const gain = ctx!.createGain();
    osc.type = "triangle";
    osc.frequency.setValueAtTime(freq, t0);
    gain.gain.setValueAtTime(0, t0);
    gain.gain.linearRampToValueAtTime(0.22, t0 + 0.012);
    gain.gain.exponentialRampToValueAtTime(0.0001, t0 + length);
    osc.connect(gain).connect(ctx!.destination);
    osc.start(t0);
    osc.stop(t0 + length + 0.02);
  });
}

export const sounds = {
  join: () => play([659.25, 987.77]), // mi -> si
  leave: () => play([783.99, 523.25]), // sol -> dó
  live: () => play([523.25, 659.25, 783.99, 1046.5], 0.07, 0.14), // dó maior
};
