import { Encoder, Numeric, binarize, Decoder, Detector, grayscale } from '@nuintun/qrcode';
import { TypedEventTarget } from "typescript-event-target";

export class QRCodeStream {

    static White = '#fff';
    static Black = '#000';

    private encoder: Encoder;
    private width: number;
    private height: number;
    private ctx: CanvasRenderingContext2D;
    private qrSize: number;
    private qrMarginX: number;
    private qrMarginY: number;

    private scheduled = false;
    private frameRequestCallback: FrameRequestCallback;

    private capturing: MediaStream | null = null;

    constructor(public canvas: HTMLCanvasElement) {
        this.encoder = new Encoder({ level: 'L' });
        const { width, height } = canvas;
        this.width = width;
        this.height = height;
        this.ctx = this.canvas.getContext('2d')!;
        this.qrSize = Math.min(width, height);
        this.qrMarginX = (width - this.qrSize) / 2;
        this.qrMarginY = (height - this.qrSize) / 2;
        this.frameRequestCallback = this.frameRequestCallback_unbound.bind(this);
    }

    private doFrame(now: number) {
        const str = now.toString();
        const qr = this.encoder.encode(new Numeric(str));
        this.ctx.fillStyle = QRCodeStream.White;
        this.ctx.fillRect(0, 0, this.width, this.height);
        this.ctx.fillStyle = QRCodeStream.Black;
        const bitmapSize = qr.size;
        const pixelSize = Math.max(1, Math.trunc(this.qrSize / bitmapSize));
        for (let i = 0; i < bitmapSize; i++) {
            for (let j = 0; j < bitmapSize; j++) {
                if (qr.get(i, j) === 1) {
                    this.ctx.fillRect(
                        this.qrMarginX + pixelSize * i,
                        this.qrMarginY + pixelSize * j,
                        pixelSize, pixelSize
                    );
                }
            }
        }
    }

    private frameRequestCallback_unbound(_time: DOMHighResTimeStamp) {
        const now = Date.now();
        this.doFrame(now);
        if (this.scheduled) {
            window.requestAnimationFrame(this.frameRequestCallback);
        }
    }

    public capture() {
        if (this.capturing) {
            return this.capturing;
        }
        if (!this.scheduled) {
            this.scheduled = true;
            window.requestAnimationFrame(this.frameRequestCallback);
        }
        const ms = this.canvas.captureStream();
        this.capturing = ms;
        return ms;
    }

    public stop() {
        if (this.scheduled) {
            this.scheduled = false;
        }
        if (this.capturing) {
            this.capturing.getTracks().forEach(t => t.stop());
            this.capturing = null;
        }
    }

}

const QRCodeStreamDecoderEvents = {
    Latency: 'latency'
} as const;

interface QRCodeStreamDecoderEventMap {
    [QRCodeStreamDecoderEvents.Latency]: CustomEvent<number>;
}

export class QRCodeStreamDecoder extends TypedEventTarget<QRCodeStreamDecoderEventMap> {

    private detector: Detector;
    private decoder: Decoder;
    private canvas: OffscreenCanvas;
    private ctx: OffscreenCanvasRenderingContext2D;

    private scheduled = false;
    private videoFrameCallback: VideoFrameRequestCallback;

    private seq = 0;

    constructor(public video: HTMLVideoElement) {
        super();
        this.detector = new Detector();
        this.decoder = new Decoder();
        this.canvas = new OffscreenCanvas(video.width, video.height);
        this.ctx = this.canvas.getContext('2d', { willReadFrequently: true })!;
        this.videoFrameCallback = this.videoFrameCallback_unbound.bind(this);
    }

    private emitLatencyEvent(detail: number) {
        this.dispatchTypedEvent(
            QRCodeStreamDecoderEvents.Latency,
            new CustomEvent(QRCodeStreamDecoderEvents.Latency, { detail })
        );
    }

    // TODO: move decoding to worker
    private decodeFrame(now: number, width: number, height: number) {
        this.canvas.width = width;
        this.canvas.height = height;
        this.ctx.drawImage(this.video, 0, 0, width, height);
        const imageData = this.ctx.getImageData(0, 0, width, height);
        const luminances = grayscale(imageData);
        const binarized = binarize(luminances, width, height);
        // assume only one QR Code per frame
        const detected = this.detector.detect(binarized).next().value;
        if (detected) {
            const decoded = this.decoder.decode(detected.matrix);
            const latency = now - Number.parseInt(decoded.content, 10);
            this.emitLatencyEvent(latency);
        }
    }

    private videoFrameCallback_unbound(_now: DOMHighResTimeStamp, metadata: VideoFrameCallbackMetadata) {
        this.seq++;
        // TODO: make it configurable
        if (this.seq >= 5) {
            this.seq = 0;
            const now = Date.now();
            this.decodeFrame(now, metadata.width, metadata.height);
        }
        if (this.scheduled) {
            this.video.requestVideoFrameCallback(this.videoFrameCallback);
        }
    }

    public start() {
        if (this.scheduled) {
            return;
        }
        this.scheduled = true;
        this.video.requestVideoFrameCallback(this.videoFrameCallback);
    }

    public stop() {
        if (!this.scheduled) {
            return;
        }
        this.scheduled = false;
    }
}
