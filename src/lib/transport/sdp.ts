// Ajustes no SDP das conexões WebRTC.

export const AUDIO_BITRATE = 128_000;

/** Liga áudio estéreo e com bitrate alto no Opus (o padrão é voz mono). */
export function tuneOpus(sdp: string): string {
  const pt = sdp.match(/a=rtpmap:(\d+) opus\/48000\/2/i)?.[1];
  if (!pt) return sdp;
  const extra = `stereo=1;sprop-stereo=1;maxaveragebitrate=${AUDIO_BITRATE}`;
  const fmtp = new RegExp(`a=fmtp:${pt} ([^\\r\\n]*)`);
  if (fmtp.test(sdp)) {
    return sdp.replace(fmtp, (line, params: string) =>
      params.includes("stereo=1") ? line : `a=fmtp:${pt} ${params};${extra}`,
    );
  }
  return sdp.replace(
    new RegExp(`(a=rtpmap:${pt} [^\\r\\n]*\\r\\n)`),
    `$1a=fmtp:${pt} ${extra}\r\n`,
  );
}
