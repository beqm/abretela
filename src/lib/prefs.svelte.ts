// Preferências do app (idioma e tema), reativas e salvas no localStorage.

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

function load(): { lang: Lang; theme: Theme } {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    return {
      lang: saved.lang in DICTS ? saved.lang : detectLang(),
      theme: ["system", "light", "dark"].includes(saved.theme) ? saved.theme : "system",
    };
  } catch {
    return { lang: detectLang(), theme: "system" };
  }
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
