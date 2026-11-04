import { describe, expect, it } from "vitest";
import { tuneOpus } from "./sdp";

const EXTRA = "stereo=1;sprop-stereo=1;maxaveragebitrate=128000";

describe("tuneOpus", () => {
  it("completa a linha fmtp do Opus", () => {
    const sdp = "a=rtpmap:111 opus/48000/2\r\na=fmtp:111 minptime=10;useinbandfec=1\r\n";
    expect(tuneOpus(sdp)).toBe(`a=rtpmap:111 opus/48000/2\r\na=fmtp:111 minptime=10;useinbandfec=1;${EXTRA}\r\n`);
  });

  it("cria a linha fmtp se não existir", () => {
    const sdp = "a=rtpmap:111 opus/48000/2\r\na=rtpmap:0 PCMU/8000\r\n";
    expect(tuneOpus(sdp)).toBe(`a=rtpmap:111 opus/48000/2\r\na=fmtp:111 ${EXTRA}\r\na=rtpmap:0 PCMU/8000\r\n`);
  });

  it("não mexe se já estiver em estéreo", () => {
    const sdp = "a=rtpmap:111 opus/48000/2\r\na=fmtp:111 stereo=1\r\n";
    expect(tuneOpus(sdp)).toBe(sdp);
  });

  it("não mexe em SDP sem Opus", () => {
    const sdp = "a=rtpmap:96 VP8/90000\r\n";
    expect(tuneOpus(sdp)).toBe(sdp);
  });
});
