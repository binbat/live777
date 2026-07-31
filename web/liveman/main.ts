import { createVaporApp, type VaporComponent } from "vue";

import "@/shared/tailwind.css";

import Liveman from "./liveman.vue";

// Admin is Vapor-compiled on this branch; the SFC d.ts still carries the
// VDOM component type, so assert the actual runtime flavor here.
createVaporApp(Liveman as unknown as VaporComponent).mount("#app");
