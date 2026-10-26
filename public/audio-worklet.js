// Toca PCM estéreo (f32 intercalado, 48kHz) recebido pela porta, com um
// pequeno buffer para absorver variações de chegada.
const RATE = 48000 * 2; // amostras por segundo (2 canais)
const PREBUFFER = RATE * 0.04; // 40ms antes de começar
const MAX_LATENCY = RATE * 0.15; // acima de 150ms, descarta o excesso
const TARGET_AFTER_DROP = RATE * 0.05;

class PcmPlayer extends AudioWorkletProcessor {
  constructor() {
    super();
    this.buf = new Float32Array(RATE); // 1s
    this.r = 0;
    this.w = 0;
    this.size = 0;
    this.playing = false;
    this.port.onmessage = (e) => this.push(e.data);
  }

  push(data) {
    const len = this.buf.length;
    for (let i = 0; i < data.length; i++) {
      this.buf[this.w] = data[i];
      this.w = (this.w + 1) % len;
    }
    this.size = Math.min(this.size + data.length, len);
    if (this.size > MAX_LATENCY) {
      let drop = this.size - TARGET_AFTER_DROP;
      drop -= drop % 2;
      this.r = (this.r + drop) % len;
      this.size -= drop;
    }
  }

  process(_inputs, outputs) {
    const [left, right] = outputs[0];
    if (!this.playing) {
      if (this.size < PREBUFFER) return true;
      this.playing = true;
    }
    const len = this.buf.length;
    for (let i = 0; i < left.length; i++) {
      if (this.size >= 2) {
        left[i] = this.buf[this.r];
        right[i] = this.buf[(this.r + 1) % len];
        this.r = (this.r + 2) % len;
        this.size -= 2;
      } else {
        left[i] = right[i] = 0;
        this.playing = false; // acabou o buffer: espera encher de novo
      }
    }
    return true;
  }
}

registerProcessor("pcm-player", PcmPlayer);
