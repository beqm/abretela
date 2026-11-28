// O que este WebView consegue fazer com os frames da captura nativa. Muda por
// sistema: o WebView2 (Chromium, Windows) tem MediaStreamTrackGenerator, que
// transforma os frames numa faixa de vídeo para o WebRTC do próprio WebView;
// o WKWebView (macOS) não tem.

/** O WebView consegue mandar a minha tela pelo WebRTC dele? */
export const canSendFromWebView = () =>
  typeof (globalThis as { MediaStreamTrackGenerator?: unknown }).MediaStreamTrackGenerator !== "undefined";

/**
 * A sala deve usar o envio nativo (o app codifica e manda)? Sim se a pessoa
 * ligou a opção, e sempre onde o WebView não consegue mandar.
 */
export const shouldSendNatively = (preference: boolean) => preference || !canSendFromWebView();

/**
 * O WebView consegue receber a tela dos outros pelo WebRTC dele? O WebKitGTK
 * (Linux) não tem WebRTC: lá quem recebe é o app (`transport/nativeWatch.ts`).
 */
export const canReceiveInWebView = () =>
  typeof (globalThis as { RTCPeerConnection?: unknown }).RTCPeerConnection !== "undefined";

/**
 * É o sistema que escolhe a tela ou a janela (Linux, pelo portal)? Então a
 * lista traz uma fonte só, do tipo "system", e o app não tem o que mostrar
 * para escolher: devolve essa fonte. Senão, `null`.
 */
export function systemPickedSource<S extends { kind: string }>(sources: S[]): S | null {
  return sources.length === 1 && sources[0].kind === "system" ? sources[0] : null;
}

/** A captura não começou porque a pessoa fechou a janela do sistema sem escolher? */
export const isShareCancelled = (error: unknown) => String(error) === "cancelled";
