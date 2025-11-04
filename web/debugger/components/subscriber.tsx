import { useSearchParams } from "@solidjs/router";
import { createSignal, Show } from "solid-js";
import { createLogger } from "../primitive/logger";
import Datachannel from "./datachannel";
import Player from "./player";

import subscribe from "./subscribe";

const WhepLayerOptions = [
	{ value: "", text: "AUTO" },
	{ value: "q", text: "LOW" },
	{ value: "h", text: "MEDIUM" },
	{ value: "f", text: "HIGH" },
];

export default function Subscriber() {
	const [disabled, setDisabled] = createSignal(true);
	const [disabledAudio, setDisabledAudio] = createSignal(false);
	const [disabledVideo, setDisabledVideo] = createSignal(false);
	const [stream, setStream] = createSignal<MediaStream | null>(null);
	const [datachannel, setDatachannel] = createSignal<RTCDataChannel | null>(
		null,
	);
	const [logs, setLogs, clear] = createLogger();

	const [audioTrackCount, setAudioTrackCount] = createSignal(0);
	const [videoTrackCount, setVideoTrackCount] = createSignal(0);

	const [searchParams] = useSearchParams();
	let stop: () => Promise<void> | undefined;
	// biome-ignore lint/suspicious/noExplicitAny: This whip-whep.js use any type
	let mute: (muted: any) => Promise<void> | undefined;
	let selectLayer: (layer: string) => Promise<void> | undefined;

	const start = async () => {
		clear();
		[stop, mute, selectLayer] = await subscribe({
			url: `${location.origin}/whep/${searchParams.id || "-"}`,
			token: (searchParams.token as string) || "",
			onStream: (stream: MediaStream | null): void => {
				setAudioTrackCount(stream ? stream.getAudioTracks().length : 0);
				setVideoTrackCount(stream ? stream.getVideoTracks().length : 0);
				setStream(stream);
			},
			onChannel: (channel: RTCDataChannel): void => {
				setDatachannel(channel);
			},
			log: setLogs,
		});
		setDisabled(false);
	};

	return (
		<>
			<legend>WHEP</legend>
			<div style="text-align: center;">
				<section>
					SVC Layer:{" "}
					<select
						disabled={disabled()}
						onChange={(e) => selectLayer(e.target.value)}
					>
						{WhepLayerOptions.map((o) => (
							<option value={o.value}>{o.text}</option>
						))}
					</select>
				</section>
				<section>
					<button
						type="button"
						disabled={disabled()}
						onClick={() => {
							const disabled = disabledAudio();
							setDisabledAudio(!disabled);
							mute({ kind: "audio", enabled: disabled });
						}}
					>
						{disabledAudio() ? "Enable" : "Disable"} Audio
					</button>
					<button
						type="button"
						disabled={disabled()}
						onClick={() => {
							const disabled = disabledVideo();
							setDisabledVideo(!disabled);
							mute({ kind: "video", enabled: disabled });
						}}
					>
						{disabledVideo() ? "Enable" : "Disable"} Video
					</button>
				</section>
				<section>
					<button type="button" onClick={start} disabled={!disabled()}>
						Start
					</button>
					<button
						type="button"
						onClick={() => {
							stop();
							setDisabled(true);
						}}
						disabled={disabled()}
					>
						Stop
					</button>
				</section>

				<section>
					<h3>WHEP Video:</h3>
					<h5>
						Audio Track Count: {audioTrackCount()}, Video Track Count:{" "}
						{videoTrackCount()}
					</h5>
					<Show when={stream()}>{(s) => <Player stream={s()} />}</Show>
				</section>
				<section>
					<Show when={datachannel()}>
						{(dc) => <Datachannel datachannel={dc()} />}
					</Show>
				</section>
				<section>
					<h4>Logs:</h4>
					<pre>{logs().join("\n")}</pre>
				</section>
			</div>
		</>
	);
}
