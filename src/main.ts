import { mount } from "svelte";
import "@fontsource-variable/nunito";
import App from "./App.svelte";
import "./styles.css";

mount(App, { target: document.getElementById("root")! });
