import { createVaporApp, type VaporComponent } from "vue";
import Debugger from "./components/debugger.vue";
import "@binbat/whep-player-vue/style.css";
import "./index.css";

// The debugger is Vapor-compiled on this branch; its d.ts still carries the
// VDOM component type, so assert the actual runtime flavor here.
createVaporApp(Debugger as unknown as VaporComponent).mount("#app");
