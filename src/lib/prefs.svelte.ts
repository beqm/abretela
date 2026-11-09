// Preferências do app (nome, idioma, tema...), reativas e salvas no localStorage.

import { pt, type Messages } from "./locales/pt";
import { en } from "./locales/en";
import { es } from "./locales/es";
import { zh } from "./locales/zh";
import { ko } from "./locales/ko";
import { ja } from "./locales/ja";

export const LANGUAGES = [
  { code: "pt", name: "Português" },
  { code: "en", name: "English" },
  { code: "es", name: "Español" },
  { code: "zh", name: "中文" },
  { code: "ko", name: "한국어" },
  { code: "ja", name: "日本語" },
] as const;

export type Lang = (typeof LANGUAGES)[number]["code"];
export type Theme = "system" | "light" | "dark";

const DICTS: Record<Lang, Messages> = { pt, en, es, zh, ko, ja };
const KEY = "abretela.prefs";

function detectLang(): Lang {
  const nav = (navigator.language || "pt").slice(0, 2).toLowerCase();
  return (LANGUAGES.find((l) => l.code === nav)?.code ?? "pt") as Lang;
}

type Prefs = {
  name: string;
  lang: Lang;
  theme: Theme;
  closeToTray: boolean;
  sounds: boolean;
  hwAccel: boolean;
  /** Envio nativo (experimental). */
  nativeSend: boolean;
};

/** Nome salvo pela tela inicial em versões antigas (até a 0.2.0). */
function legacyName(): string {
  try {
    return JSON.parse(localStorage.getItem("abretela.name") ?? "{}").name ?? "";
  } catch {
    return "";
  }
}

function load(): Prefs {
  let saved: Partial<Prefs> = {};
  try {
    saved = JSON.parse(localStorage.getItem(KEY) ?? "{}");
  } catch {}
  return {
    name: typeof saved.name === "string" ? saved.name : legacyName(),
    lang: saved.lang && saved.lang in DICTS ? saved.lang : detectLang(),
    theme: saved.theme && ["system", "light", "dark"].includes(saved.theme) ? saved.theme : "system",
    closeToTray: saved.closeToTray ?? true,
    sounds: saved.sounds ?? true,
    hwAccel: saved.hwAccel ?? true,
    nativeSend: saved.nativeSend ?? false,
  };
}

const state = $state(load());

function apply() {
  document.documentElement.lang = state.lang === "zh" ? "zh-CN" : state.lang;
  if (state.theme === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = state.theme;
  try {
    localStorage.setItem(KEY, JSON.stringify(state));
  } catch {}
}
apply();

export const prefs = {
  /** Como os outros te veem nas salas (vazio = ainda não escolheu). */
  get name() {
    return state.name;
  },
  set name(v: string) {
    const name = v.trim().slice(0, 32);
    if (!name) return;
    state.name = name;
    apply();
  },
  get lang() {
    return state.lang;
  },
  set lang(v: Lang) {
    state.lang = v;
    apply();
  },
  get theme() {
    return state.theme;
  },
  set theme(v: Theme) {
    state.theme = v;
    apply();
  },
  get closeToTray() {
    return state.closeToTray;
  },
  set closeToTray(v: boolean) {
    state.closeToTray = v;
    apply();
  },
  get sounds() {
    return state.sounds;
  },
  set sounds(v: boolean) {
    state.sounds = v;
    apply();
  },
  /** Placa de vídeo na conversão da captura e na codificação (H.264). */
  get hwAccel() {
    return state.hwAccel;
  },
  set hwAccel(v: boolean) {
    state.hwAccel = v;
    apply();
  },
  /** Envio nativo (ROADMAP, seção 2, experimental): minha tela é codificada e
   * mandada pelo próprio app. Vale a partir da próxima sala. */
  get nativeSend() {
    return state.nativeSend;
  },
  set nativeSend(v: boolean) {
    state.nativeSend = v;
    apply();
  },
};

export type MessageKey = keyof Messages;

/** Traduz uma chave; `{nome}` no texto é trocado por `params.nome`. */
export function t(key: MessageKey, params?: Record<string, string | number>): string {
  let text = DICTS[state.lang][key] ?? pt[key];
  if (params) {
    for (const [k, v] of Object.entries(params)) text = text.replaceAll(`{${k}}`, String(v));
  }
  return text;
}
