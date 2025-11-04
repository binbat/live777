import { createEffect, createSignal, on } from "solid-js";

const NoneDevice = { value: "", text: "none" };

function deviceInfoToOption(info: MediaDeviceInfo) {
	const value = info.deviceId;
	let text = info.label;
	if (text.length <= 0) {
		text = `${info.kind} (${info.deviceId})`;
	}
	return { value, text };
}

function uniqByValue<T extends { value: unknown }>(items: T[]) {
	const map = new Map<unknown, T>();
	for (const item of items) {
		if (!map.has(item.value)) {
			map.set(item.value, item);
		}
	}
	return Array.from(map.values());
}

export default function Device(props: {
	disabled: boolean;
	onSelectAudio: (deviceId: string) => void;
	onSelectVideo: (deviceId: string) => void;
}) {
	//const [refreshDisabled, setRefreshDisabled] = createSignal(false);
	const [audioDevices, setAudioDevices] = createSignal([NoneDevice]);
	const [videoDevices, setVideoDevices] = createSignal([NoneDevice]);
	//const [searchParams, setSearchParams] = useSearchParams();

	const refreshDevice = async () => {
		try {
			// to obtain non-empty device label, there needs to be an active media stream or persistent permission
			// https://developer.mozilla.org/en-US/docs/Web/API/MediaDeviceInfo/label#value
			const mediaStream = await navigator.mediaDevices.getUserMedia({
				audio: true,
				video: true,
			});
			const devices = (await navigator.mediaDevices.enumerateDevices()).filter(
				(i) => !!i.deviceId,
			);
			mediaStream.getTracks().map((track) => track.stop());
			const audio = devices
				.filter((i) => i.kind === "audioinput")
				.map(deviceInfoToOption);
			if (audio.length > 0) {
				setAudioDevices(uniqByValue(audio));
				//props.onSelectAudio(audioDevices[0].deviceId)
			}
			const video = devices
				.filter((i) => i.kind === "videoinput")
				.map(deviceInfoToOption);
			if (video.length > 0) {
				setVideoDevices(uniqByValue(video));
			}
		} catch (e) {
			console.error("refreshDevice failed:", e);
		}

		// TODO: 应该放到启动时在禁用
		//setRefreshDisabled(true);
	};
	createEffect(
		on(audioDevices, (i) => i.length > 0 && props.onSelectAudio(i[0].value), {
			defer: true,
		}),
	);
	createEffect(
		on(videoDevices, (i) => i.length > 0 && props.onSelectVideo(i[0].value), {
			defer: true,
		}),
	);

	return (
		<>
			<button type="button" disabled={props.disabled} onClick={refreshDevice}>
				Use Device
			</button>
			<div style="margin: 0.2rem">
				Audio Device:
				<select
					onChange={(e) => {
						props.onSelectAudio(e.target.value);
					}}
				>
					{audioDevices().map((d) => (
						<option value={d.value}>{d.text}</option>
					))}
				</select>
			</div>
			<div style="margin: 0.2rem">
				Video Device:
				<select
					onChange={(e) => {
						props.onSelectVideo(e.target.value);
					}}
				>
					{videoDevices().map((d) => (
						<option value={d.value}>{d.text}</option>
					))}
				</select>
			</div>
		</>
	);
}
