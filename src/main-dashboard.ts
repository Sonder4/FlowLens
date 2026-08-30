import "./app.css";
import { mount } from "svelte";
import App from "./App.svelte";

function showErr(msg: string): void {
  const el = document.createElement("pre");
  el.style.cssText =
    "position:fixed;top:0;left:0;z-index:9999;color:#b00;background:#fff;padding:10px;font-size:12px;white-space:pre-wrap;max-width:100%;max-height:60vh;overflow:auto";
  el.textContent = msg;
  document.body.appendChild(el);
}

const origError = console.error;
console.error = (...args: unknown[]): void => {
  showErr("CONSOLE: " + args.map(String).join(" "));
  origError(...args);
};

window.addEventListener("error", (e) => {
  showErr("ERR: " + ((e as ErrorEvent).error?.stack || e.message));
});
window.addEventListener("unhandledrejection", (e) => {
  showErr("REJ: " + String((e as PromiseRejectionEvent).reason?.stack || e.reason));
});

const app = mount(App, { target: document.getElementById("app")! });
export default app;
