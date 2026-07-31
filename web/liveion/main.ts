import { createVaporApp, type VaporComponent } from "vue";

import "../shared/tailwind.css";

import Liveion from "./liveion.vue";

// Admin is Vapor-compiled on this branch; the SFC d.ts still carries the
// VDOM component type, so assert the actual runtime flavor here.
createVaporApp(Liveion as unknown as VaporComponent).mount("#app");
