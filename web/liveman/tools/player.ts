import { StandaloneWhepPlayer } from "@binbat/whep-player-vue/vapor";
import "@binbat/whep-player-vue/style.css";
import "@/shared/player-page.css";
import { createVaporApp, type VaporComponent } from "vue";

// The ./vapor bundle is Vapor-compiled, but its shared d.ts still carries
// the VDOM component type — assert the actual runtime flavor here.
createVaporApp(StandaloneWhepPlayer as unknown as VaporComponent).mount(
    "#app",
);
