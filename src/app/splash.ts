// Janelinha de abertura: mascote + busca de atualização (ver startup.rs).

import { mount } from "svelte";
import Splash from "./Splash.svelte";
import { installGlobalLogging } from "../lib/log";

installGlobalLogging({ describeGpu: false });
document.addEventListener("contextmenu", (e) => e.preventDefault());

mount(Splash, { target: document.getElementById("root")! });
