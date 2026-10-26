import { mount } from "svelte";
import "@fontsource-variable/nunito";
import App from "./App.svelte";
import "./styles.css";

// Sem menu do navegador (Inspecionar, Recarregar...): só nos campos de texto,
// para dar para colar o convite.
document.addEventListener("contextmenu", (e) => {
  const el = e.target as HTMLElement;
  if (!el.closest("input, textarea")) e.preventDefault();
});

// Na versão final, bloqueia os atalhos de inspecionar, recarregar, imprimir etc.
if (!import.meta.env.DEV) {
  document.addEventListener(
    "keydown",
    (e) => {
      const key = e.key.toLowerCase();
      const ctrl = e.ctrlKey || e.metaKey;
      const blocked =
        key === "f12" ||
        key === "f5" ||
        key === "f7" ||
        (ctrl && e.shiftKey && ["i", "j", "c", "r"].includes(key)) ||
        (ctrl && ["r", "u", "p", "s", "f", "g", "j", "h"].includes(key));
      if (blocked) {
        e.preventDefault();
        e.stopPropagation();
      }
    },
    true,
  );
}

mount(App, { target: document.getElementById("root")! });
