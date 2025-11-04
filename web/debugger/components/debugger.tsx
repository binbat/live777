import { useSearchParams } from "@solidjs/router";
import "./debugger.css";

import Publisher from "./publisher";
import Subscriber from "./subscriber";

export default function Debugger() {
	const [searchParams, setSearchParams] = useSearchParams();

	return (
		<>
			<fieldset>
				<legend>Common</legend>
				<section style="display: flex;justify-content: space-evenly;flex-wrap: wrap;">
					<div>
						Stream ID:{" "}
						<input
							type="text"
							value={searchParams.id || ""}
							onInput={(e) => {
								setSearchParams({ id: e.target.value });
							}}
						/>
					</div>
					<div>
						Bearer Token:{" "}
						<input
							type="text"
							value={searchParams.token || ""}
							onInput={(e) => {
								setSearchParams({ token: e.target.value });
							}}
						/>
					</div>
				</section>
			</fieldset>
			<div style="display: flex;justify-content: space-evenly;flex-wrap: wrap;">
				<fieldset>
					<Publisher />
				</fieldset>
				<fieldset>
					<Subscriber />
				</fieldset>
			</div>
		</>
	);
}
