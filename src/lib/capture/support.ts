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
