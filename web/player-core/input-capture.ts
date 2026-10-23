// Remote input capture for streams whose source accepts it (the wayland
// compositor source with `input=1`): pointer and keyboard events on the
// player surface are JSON-encoded and sent over the "input" datachannel,
// which lands on the stream's `input` cable at the server. Wire format lives
// in liveion/src/stream/source/wayland/input.rs.

export type InputChannelSource = () => RTCDataChannel | null;

export type InputCaptureOptions = {
    // The element users point and click at (the video container). Keyboard
    // events are captured while it is focused (mousedown focuses it).
    target: HTMLElement;
    // Canvas size source: the video element carrying the stream. Pointer
    // coordinates are mapped from element pixels to canvas pixels.
    video: () => HTMLVideoElement | null;
    // The "input" datachannel; events are dropped while it is not open.
    channel: InputChannelSource;
};

// Attaches the listeners; returns a detach function.
export function attachInputCapture(options: InputCaptureOptions): () => void {
    const { target, video, channel } = options;

    const send = (msg: Record<string, unknown>) => {
        const ch = channel();
        if (ch?.readyState === "open") {
            ch.send(JSON.stringify(msg));
        }
    };

    const canvasPos = (e: MouseEvent) => {
        const v = video();
        const rect = target.getBoundingClientRect();
        const cw = v?.videoWidth || rect.width;
        const chh = v?.videoHeight || rect.height;
        const x = ((e.clientX - rect.left) / rect.width) * cw;
        const y = ((e.clientY - rect.top) / rect.height) * chh;
        return { x: Math.round(x * 100) / 100, y: Math.round(y * 100) / 100 };
    };

    // Browsers stream mousemove at event rate; coalesce to one message per
    // animation frame so the datachannel is not flooded.
    let movePending = false;
    let lastMove: MouseEvent | null = null;
    const flushMove = () => {
        movePending = false;
        if (!lastMove) return;
        const { x, y } = canvasPos(lastMove);
        lastMove = null;
        send({ t: "move", x, y });
    };
    const onMouseMove = (e: MouseEvent) => {
        lastMove = e;
        if (!movePending) {
            movePending = true;
            requestAnimationFrame(flushMove);
        }
    };
    const onMouseDown = (e: MouseEvent) => {
        if (e.button > 2) return;
        target.focus();
        const { x, y } = canvasPos(e);
        // A click implies the pointer position; keep the server-side pointer
        // state coherent even if the last move message is still queued.
        send({ t: "move", x, y });
        send({ t: "button", b: e.button, down: 1 });
        e.preventDefault();
    };
    // mouseup on window so a drag release outside the element still closes.
    const onMouseUp = (e: MouseEvent) => {
        if (e.button > 2) return;
        send({ t: "button", b: e.button, down: 0 });
    };
    const onWheel = (e: WheelEvent) => {
        // Normalize to wheel notches: deltaMode 0 is pixels (~53 per notch),
        // 1 is lines (= notches), 2 is pages.
        const f = e.deltaMode === 0 ? 1 / 53 : e.deltaMode === 2 ? 3 : 1;
        const round = (v: number) => Math.round(v * 100) / 100;
        send({ t: "wheel", x: round(e.deltaX * f), y: round(e.deltaY * f) });
        e.preventDefault();
    };
    const onContextMenu = (e: MouseEvent) => e.preventDefault();
    const onKey = (e: KeyboardEvent) => {
        send({ t: "key", code: e.code, down: e.type === "keydown" ? 1 : 0 });
        e.preventDefault();
    };

    if (target.tabIndex < 0) target.tabIndex = 0;
    target.addEventListener("mousemove", onMouseMove);
    target.addEventListener("mousedown", onMouseDown);
    window.addEventListener("mouseup", onMouseUp);
    target.addEventListener("wheel", onWheel, { passive: false });
    target.addEventListener("contextmenu", onContextMenu);
    target.addEventListener("keydown", onKey);
    target.addEventListener("keyup", onKey);

    return () => {
        target.removeEventListener("mousemove", onMouseMove);
        target.removeEventListener("mousedown", onMouseDown);
        window.removeEventListener("mouseup", onMouseUp);
        target.removeEventListener("wheel", onWheel);
        target.removeEventListener("contextmenu", onContextMenu);
        target.removeEventListener("keydown", onKey);
        target.removeEventListener("keyup", onKey);
    };
}
