/* @refresh reload */
import { render } from "solid-js/web";
import { Router } from "@solidjs/router";
import Debugger from "./components/debugger";
import './index.css'

render(
	() => <Router root={Debugger} />,
	document.getElementById("app") as HTMLElement,
);
