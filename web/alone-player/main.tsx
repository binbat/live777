/* @refresh reload */
import { render } from "solid-js/web";
import Player from "./player";

render(() => <Player />, document.getElementById("app") as HTMLElement);
