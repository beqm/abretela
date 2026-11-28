// Em que sistema o app está rodando, para os textos que mudam de um para o
// outro (no macOS a "bandeja" é a barra de menus).

import { isTauri } from "./env";

/** O app está rodando no macOS? */
export const isMac = () => /Macintosh|Mac OS X/.test(navigator.userAgent);

/** O app (ou a página) está rodando no Linux de um computador? O Android também se diz Linux. */
export const isLinux = () => /Linux/.test(navigator.userAgent) && !/Android/.test(navigator.userAgent);

/**
 * Ao pôr um elemento em tela cheia, a janela do app precisa ir junto? No
 * WebView2 (Windows) a tela cheia de um elemento só enche a janela. No
 * WKWebView (macOS) ela já ocupa a tela inteira, numa janela do próprio
 * sistema; pedir a tela cheia da janela ao mesmo tempo faz as duas brigarem
 * e nenhuma fica. Numa aba do navegador (a página de assistir) não há janela
 * do app para mexer.
 */
export const windowFollowsElementFullscreen = () => isTauri() && !isMac();
