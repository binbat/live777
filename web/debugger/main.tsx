/* @refresh reload */

import { Router } from "@solidjs/router";
import { render } from "solid-js/web";
import Debugger from "./components/debugger";
import "player-core/style.css";
import "./index.css";

render(
    () => <Router root={Debugger} />,
    document.getElementById("app") as HTMLElement,
);
