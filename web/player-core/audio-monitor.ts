// Framework-agnostic audio monitor: wraps an AnalyserNode fed by the audio
// tracks of a MediaStream to expose level/waveform/spectrum readings for
// debugging UIs. The analyser is deliberately not connected to the
// destination so monitoring never plays sound on its own.
export class AudioMonitor {
    private audioContext: AudioContext;
    private source: MediaStreamAudioSourceNode;
    private analyser: AnalyserNode;
    private timeDomainData: Uint8Array<ArrayBuffer>;
    private frequencyData: Uint8Array<ArrayBuffer>;
    private closed = false;

    constructor(stream: MediaStream, fftSize = 512) {
        const audioTracks = stream.getAudioTracks();
        if (audioTracks.length === 0) {
            throw new Error(
                "AudioMonitor requires a stream with an audio track",
            );
        }
        this.audioContext = new AudioContext();
        this.source = this.audioContext.createMediaStreamSource(
            new MediaStream(audioTracks),
        );
        this.analyser = this.audioContext.createAnalyser();
        this.analyser.fftSize = fftSize;
        this.analyser.smoothingTimeConstant = 0.8;
        this.timeDomainData = new Uint8Array(this.analyser.fftSize);
        this.frequencyData = new Uint8Array(this.analyser.frequencyBinCount);
        this.source.connect(this.analyser);
    }

    get state(): AudioContextState {
        return this.audioContext.state;
    }

    get frequencyBinCount(): number {
        return this.analyser.frequencyBinCount;
    }

    // Autoplay policies may start the context suspended; call from a user
    // gesture (or retry from a draw loop) to start receiving samples.
    async resume(): Promise<void> {
        if (this.audioContext.state === "suspended") {
            await this.audioContext.resume().catch(() => {});
        }
    }

    // Perceptual loudness in [0, 1]: time-domain RMS mapped through
    // log10(1 + 9x), so speech-level signals spread across most of the range.
    level(): number {
        this.analyser.getByteTimeDomainData(this.timeDomainData);
        let sum = 0;
        for (let i = 0; i < this.timeDomainData.length; i++) {
            const value = this.timeDomainData[i] - 128;
            sum += value * value;
        }
        const rms = Math.sqrt(sum / this.timeDomainData.length) / 127;
        return Math.log10(1 + 9 * Math.min(rms, 1));
    }

    waveform(): Uint8Array<ArrayBuffer> {
        this.analyser.getByteTimeDomainData(this.timeDomainData);
        return this.timeDomainData;
    }

    spectrum(): Uint8Array<ArrayBuffer> {
        this.analyser.getByteFrequencyData(this.frequencyData);
        return this.frequencyData;
    }

    close(): void {
        if (this.closed) {
            return;
        }
        this.closed = true;
        this.source.disconnect();
        this.analyser.disconnect();
        void this.audioContext.close().catch(() => {});
    }
}
