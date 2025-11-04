import { createSignal, onCleanup, onMount } from "solid-js";

export default function Datachannel(props: { datachannel: RTCDataChannel }) {
	const [datachannelState, setDatachannelState] = createSignal("");
	const [logs, setLogs] = createSignal<string[]>([]);

	const onmessage = (ev: MessageEvent) => {
		setLogs((prev) => [
			...prev,
			new TextDecoder("utf-8").decode(new Uint8Array(ev.data)),
		]);
	};

	onMount(() => {
		const onopen = () => setDatachannelState("opened");
		const onclose = () => setDatachannelState("closed");

		props.datachannel.addEventListener("message", onmessage);
		props.datachannel.addEventListener("close", onclose);
		props.datachannel.addEventListener("open", onopen);

		onCleanup(() => {
			props.datachannel.removeEventListener("open", onopen);
			props.datachannel.removeEventListener("close", onclose);
			props.datachannel.removeEventListener("message", onmessage);
		});
	});

	return (
		<>
			<div>State: {datachannelState()}</div>
			<div>
				Datachannel:{" "}
				<input
					type="text"
					onChange={(e) => {
						props.datachannel.send(e.target.value);
					}}
				/>
			</div>
			<pre>{logs().join("\n")}</pre>
		</>
	);
}
