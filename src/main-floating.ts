import "./app.css";
import { mount } from "svelte";
import Floating from "./Floating.svelte";

const app = mount(Floating, { target: document.getElementById("app")! });
export default app;
