//! Native source configuration types.
//!
//! Structured TOML config under a per-stream `[[stream.<name>.sources]]` block.
//! A source carries a video leg (`capture` + `encoder`), an audio leg
//! (`audio_capture` + `audio_encoder`), or both.  The video source type is
//! determined by `capture.backend`:
//!
//! ```toml
//! [stream.usb-cam]
//! [[stream.usb-cam.sources]]
//!
//! [stream.usb-cam.sources.capture]
//! backend = "v4l2"
//! device = "/dev/video0"
//! width = 640
//! height = 480
//! fps = 30
//! pixel_format = "yuyv"
//!
//! [stream.usb-cam.sources.encoder]
//! backend = "rdk"
//! codec = "h264"
//! bitrate = 1_500_000
//! profile = "42001f"
//! gop = 60
//! # bitrate_mode = "vbr"   # v4l2-m2m only: "vbr" (driver default) or "cbr";
//!                          # bcm2835-codec firmware rejects CBR
//!
//! [stream.usb-cam.sources.output]
//! payload_type = 96
//! clock_rate = 90000
//! ```
//!
//! The camera-free mock capture backend (issue #519) synthesizes NV12
//! frames — 75% color bars, a bouncing block and a binary frame counter —
//! so the hardware encoder path can be brought up with no camera attached.
//! It opens no device:
//!
//! ```toml
//! [stream.encoder-test]
//! [[stream.encoder-test.sources]]
//!
//! [stream.encoder-test.sources.capture]
//! backend = "mock"
//! width = 1280
//! height = 720
//! fps = 30
//! pixel_format = "nv12"
//! # pattern = "smpte75"   # background: smpte75 (default) / snow / black /
//!                         # white / checkers / gradient / zone-plate
//!
//! [stream.encoder-test.sources.encoder]
//! backend = "rkmpp"
//! codec = "h264"
//! bitrate = 2_000_000
//! profile = "640028"
//! ```
//!
//! An audio leg (microphone → Opus, or the `sine` test-tone generator) —
//! alone for an audio-only stream, or alongside the video leg.  Requires
//! the `encoder-opus` feature:
//!
//! ```toml
//! [stream.mic]
//! [[stream.mic.sources]]
//!
//! [stream.mic.sources.audio_capture]
//! backend = "alsa"            # or "sine" (synthetic 440 Hz test tone)
//! device = "default"          # optional, default "default"
//! sample_rate = 48000         # optional, only 48000 accepted in v1
//! channels = 2                # optional, 1|2
//!
//! [stream.mic.sources.audio_encoder]
//! codec = "opus"
//! bitrate = 64000             # optional default
//! application = "audio"       # optional: audio|voip|lowdelay
//! frame_ms = 20               # optional: 5|10|20|40|60
//! vbr = true                  # optional
//! inband_fec = false          # optional
//! dtx = false                 # optional
//! complexity = 9              # optional 0..=10
//! adaptive = false            # optional, same untagged shape as encoder.adaptive
//! ```

use serde::{Deserialize, Serialize};

#[cfg(feature = "native-source")]
use livehal::NativeSourceParams;

// ---------------------------------------------------------------------------
// Structured source configuration types (v2 — recommended)
// ---------------------------------------------------------------------------

/// Capture (input device) specification.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CaptureSpec {
    /// Capture backend: `"libcamera"`, `"v4l2"`, or `"mock"` (synthetic
    /// NV12 test frames; opens no device — for camera-free encoder
    /// testing, issue #519).
    pub backend: String,
    /// Capture device identifier.
    /// - For `libcamera`: camera ID, e.g. `"0"`.
    /// - For `v4l2`: device path, e.g. `"/dev/video0"`.
    /// - For `mock`: unused; frames are synthesized.
    #[serde(default)]
    pub device: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Raw pixel format: `"yuyv"`, `"uyvy"`, `"nv12"`, `"yuv420"`,
    /// `"rgb888"` (compressed `"mjpeg"` is rejected end-to-end).
    pub pixel_format: String,
    /// Prefer DMA-BUF zero-copy path (default `false`).
    #[serde(default)]
    pub prefer_dmabuf: bool,
    /// Horizontal flip (mirror the image left-right, default `false`).
    /// Both `hflip` and `vflip` set together give a 180° rotation.
    ///
    /// **libcamera backend only**: the flip is applied by the ISP, so it
    /// costs no CPU.  90°/270° rotation is not offered — the Raspberry Pi
    /// ISP cannot transpose an image in hardware.  The `v4l2` backend is
    /// rejected by validation: its video node (unicam) only DMA-moves
    /// frames and has no image-processing capability.
    #[serde(default)]
    pub hflip: bool,
    /// Vertical flip (top-bottom, default `false`).  See [`CaptureSpec::hflip`].
    #[serde(default)]
    pub vflip: bool,
    /// Background pattern for the synthetic `mock` capture backend:
    /// `"smpte75"` (default), `"snow"`, `"black"`, `"white"`, `"checkers"`,
    /// `"gradient"` — names mirror gst `videotestsrc`.  The bouncing block,
    /// frame counter strip and frame-number burn-in stay on top of every
    /// pattern.  Must be unset for real capture backends.
    #[serde(default)]
    pub pattern: Option<String>,
}

/// Encoder specification.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EncoderSpec {
    /// Encoder backend: `"v4l2-m2m"`, `"rdk"`, `"rkmpp"`, or `"cedar"`.
    pub backend: String,
    /// Video codec: `"h264"` or `"h265"`.
    pub codec: String,
    /// Target bitrate in bits per second.
    pub bitrate: u32,
    /// Codec profile identifier.
    /// For H.264 this is the profile/level-id hex string, e.g. `"42001f"`.
    pub profile: String,
    /// Optional explicit level (H.264/H.265 level-id component).
    /// When omitted, `profile` is treated as the complete profile-level-id.
    #[serde(default)]
    pub level: Option<String>,
    /// Optional encoder tier (H.265 only: `"main"` or `"high"`).
    #[serde(default)]
    pub tier: Option<String>,
    /// GOP size (keyframe interval in frames).  `0` disables periodic
    /// keyframes on the `v4l2-m2m` backend: the encoder emits one initial
    /// IDR and afterwards only P-frames, so keyframes are produced solely
    /// on request (subscriber join / RTCP PLI/FIR → `FORCE_KEY_FRAME`).
    #[serde(default = "default_gop")]
    pub gop: u32,
    /// Encoder rate-control mode: `"vbr"` or `"cbr"`.  Absent keeps the
    /// driver default (VBR on bcm2835-codec).  Only the `v4l2-m2m` backend
    /// honours this today; other backends ignore it.  Note the
    /// bcm2835-codec (Raspberry Pi) firmware rejects CBR — the encoder
    /// fails to start with it.
    #[serde(default)]
    pub bitrate_mode: Option<String>,
    /// Prefer DMA-BUF zero-copy path (default `false`).
    #[serde(default)]
    pub prefer_dmabuf: bool,
    /// Adaptive bitrate (issue #409) — **on by default, no key needed**.
    /// The controller lowers the running encoder's target while WHEP
    /// subscribers report congestion and ramps it back up when the links
    /// stay clean; a clean link is never touched, so a fixed-looking rate
    /// is just an AIMD that never triggers.  Effective only with encoder
    /// backends that support runtime retuning (currently `rkmpp` and
    /// `v4l2-m2m`; others log a warning and disable it).
    ///
    /// Three self-describing forms:
    /// - absent: enabled; the floor is the lowest tier's bitrate when
    ///   tiers are declared, else `max(bitrate / 8, 300_000)`.
    /// - `adaptive = false`: strictly fixed rate (no AIMD).
    /// - `adaptive = { min_bitrate = 150000 }`: enabled, custom floor.
    #[serde(default)]
    pub adaptive: Option<AdaptiveConfig>,
}

/// The `adaptive` encoder key — see [`EncoderSpec::adaptive`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AdaptiveConfig {
    /// Plain switch: `adaptive = false` disables; `adaptive = true` is
    /// the same as omitting the key.
    Switch(bool),
    /// Knobs: `adaptive = { min_bitrate = 150000 }`.
    Knobs {
        /// Adaptive floor in bits per second (default: lowest tier, else
        /// `max(bitrate / 8, 300_000)`).
        #[serde(default)]
        min_bitrate: Option<u32>,
    },
}

fn default_gop() -> u32 {
    60
}

/// Audio capture (input device) specification — see the module header for
/// the TOML shape.  Every field except `backend` has a default, so the
/// minimal form is `backend = "alsa"` (or `"sine"`) alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioCaptureSpec {
    /// Capture backend: `"alsa"` (microphone) or `"sine"` (synthetic
    /// 440 Hz test tone, no sound card required).
    pub backend: String,
    /// Capture device identifier (the ALSA PCM device, e.g. `"default"`;
    /// unused by the `sine` backend).
    #[serde(default = "default_audio_device")]
    pub device: String,
    /// Capture sample rate; v1 captures at 48000 Hz only (no resampling).
    #[serde(default = "default_audio_sample_rate")]
    pub sample_rate: u32,
    /// Channel count: 1 or 2.
    #[serde(default = "default_audio_channels")]
    pub channels: u32,
}

/// Audio encoder (Opus) specification — see the module header for the TOML
/// shape.  Every field except `codec` has a default, so the minimal form is
/// `codec = "opus"` alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioEncoderSpec {
    /// Audio codec: `"opus"` only (fixed wire contract: Opus 48 kHz, RTP
    /// payload type 111, clock 48000, RFC 7587).
    pub codec: String,
    /// Opus bitrate in bits per second (6_000..=510_000, default 64000).
    #[serde(default = "default_audio_bitrate")]
    pub bitrate: u32,
    /// Opus application mode: `"audio"` (default), `"voip"` or `"lowdelay"`.
    #[serde(default = "default_audio_application")]
    pub application: String,
    /// Opus frame size in ms: 5 | 10 | 20 (default) | 40 | 60.
    #[serde(default = "default_audio_frame_ms")]
    pub frame_ms: u32,
    /// Variable bitrate (default `true`; `false` = CBR).
    #[serde(default = "default_audio_vbr")]
    pub vbr: bool,
    /// Opus in-band FEC (default `false`).
    #[serde(default)]
    pub inband_fec: bool,
    /// Opus discontinuous transmission (default `false`).
    #[serde(default)]
    pub dtx: bool,
    /// Opus encoder complexity, 0..=10 (default 9).
    #[serde(default = "default_audio_complexity")]
    pub complexity: u32,
    /// Adaptive bitrate — the same untagged shape as
    /// [`EncoderSpec::adaptive`], on by default; the floor defaults to the
    /// lowest tier's bitrate when tiers are declared, else
    /// `max(bitrate / 4, 16_000)`.  Ignored when the source also has a
    /// video leg: the shared AIMD controller then drives the video
    /// encoder (mirroring the tier rule, which targets video first).
    #[serde(default)]
    pub adaptive: Option<AdaptiveConfig>,
}

fn default_audio_device() -> String {
    "default".to_owned()
}

fn default_audio_sample_rate() -> u32 {
    48_000
}

fn default_audio_channels() -> u32 {
    2
}

fn default_audio_bitrate() -> u32 {
    64_000
}

fn default_audio_application() -> String {
    "audio".to_owned()
}

fn default_audio_frame_ms() -> u32 {
    20
}

fn default_audio_vbr() -> bool {
    true
}

fn default_audio_complexity() -> u32 {
    9
}

/// RTP output specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputSpec {
    /// RTP payload type (default `96`).
    #[serde(default = "default_payload_type")]
    pub payload_type: u8,
    /// RTP clock rate in Hz (default `90000`).
    #[serde(default = "default_clock_rate")]
    pub clock_rate: u32,
}

/// A named quality tier — a preset group of source parameters the stream
/// can be switched to through the admin API
/// (`POST /api/sources/{stream}/tier`).
///
/// A tier is a partial overlay on the source's own structure: optional
/// `capture` and `encoder` blocks holding the parameters to change —
/// anything left unset keeps the configured value.  Tiers sit at *source*
/// level because a rung spans both blocks: retuning the encoder
/// (`bitrate`) while reconfiguring the capture (`width`/`height`/`fps`,
/// livehal has no scaler stage — capture size is encoder input size, and
/// framerate is a capture-side property).  A tier that changes capture
/// geometry switches by rebuilding the capture+encoder pipeline
/// (sub-second frame gap); an encoder-only change retunes the running
/// encoder seamlessly.  The same shape can later grow to switch more of
/// the source (device, profile, or even the source type).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TierSpec {
    /// Unique tier name within the source, referenced by the admin API.
    pub name: String,
    /// Capture-parameter overlay (absent fields keep the configured
    /// capture).  Changing any of these rebuilds the pipeline.
    #[serde(default)]
    pub capture: TierCaptureSpec,
    /// Encoder-parameter overlay (absent fields keep the configured
    /// encoder).  Retunes in place when it is the only change.
    #[serde(default)]
    pub encoder: TierEncoderSpec,
}

/// The `capture` block of a [`TierSpec`] — capture-side parameters a tier
/// may override (see [`TierSpec`]).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TierCaptureSpec {
    /// Tier capture width (must pair with `height`; any size the sensor
    /// supports, including above the boot profile — e.g. a 972p30 boot
    /// with a 1080p20 max-quality rung).
    #[serde(default)]
    pub width: Option<u32>,
    /// Tier capture height (must pair with `width`; see `width`).
    #[serde(default)]
    pub height: Option<u32>,
    /// Tier capture framerate.  May exceed `capture.fps` when the sensor
    /// supports it (e.g. a low-latency rung trading resolution for
    /// framerate on an OV5647: 1080p30 base, 480p60 rung); a mode the
    /// camera cannot do is rejected at apply time and rolled back.
    #[serde(default)]
    pub fps: Option<u32>,
}

/// The `encoder` block of a [`TierSpec`] — encoder-side parameters a tier
/// may override (see [`TierSpec`]).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TierEncoderSpec {
    /// Tier bitrate in bits per second — the AIMD's ceiling in this rung.
    /// Optional: when unset it is derived from the tier's effective
    /// geometry at ~0.07 bits per pixel (see [`TierSpec::derived_bitrate`]),
    /// which covers the common case; set it explicitly to tune for content
    /// (e.g. sports) or to offer premium/economy rungs at the same
    /// geometry.  Bounded only by the encoder's signed-32-bit control
    /// channel, so a rung may exceed the boot `encoder.bitrate`.
    #[serde(default)]
    pub bitrate: Option<u32>,
}

impl TierSpec {
    /// Geometry-derived default bitrate: ~0.07 bits per pixel per frame
    /// (where the hand-tuned OV5647 ladder lands), floored at 50 kbps.
    pub fn derived_bitrate(width: u32, height: u32, fps: u32) -> u32 {
        ((width as u64 * height as u64 * fps as u64 * 7 / 100) as u32).max(50_000)
    }

    /// Whether this tier overrides capture geometry (such a rung rebuilds
    /// the video capture+encoder pipeline at apply time).
    pub fn has_geometry(&self) -> bool {
        self.capture.width.is_some() || self.capture.height.is_some() || self.capture.fps.is_some()
    }
}

fn default_payload_type() -> u8 {
    96
}

fn default_clock_rate() -> u32 {
    90000
}

/// Full specification for a single media source.
///
/// This is the recommended structured config format replacing the
/// URL query-string approach.  A source carries a video leg
/// (`capture` + `encoder`), an audio leg (`audio_capture` +
/// `audio_encoder`), or both; each leg must be complete or absent
/// entirely (half-legs are rejected by [`SourceSpec::validate`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSpec {
    /// Unique stream identifier.
    pub stream_id: String,
    /// Video leg: capture / input device configuration.
    #[serde(default)]
    pub capture: Option<CaptureSpec>,
    /// Video leg: encoder configuration.
    #[serde(default)]
    pub encoder: Option<EncoderSpec>,
    /// Audio leg: capture configuration (microphone / test tone).
    #[serde(default)]
    pub audio_capture: Option<AudioCaptureSpec>,
    /// Audio leg: Opus encoder configuration.
    #[serde(default)]
    pub audio_encoder: Option<AudioEncoderSpec>,
    /// RTP output parameters (video leg).
    #[serde(default)]
    pub output: OutputSpec,
    /// Optional named quality tiers (bitrate presets; resolution/framerate
    /// tiers reserved).  See [`TierSpec`] for why this lives at source
    /// level rather than inside the encoder block.  `tier.encoder.bitrate`
    /// targets the video encoder when a video leg exists, else the audio
    /// encoder; capture-geometry fields are invalid on an audio-only
    /// source.
    #[serde(default)]
    pub tiers: Vec<TierSpec>,
}

impl Default for OutputSpec {
    fn default() -> Self {
        Self {
            payload_type: default_payload_type(),
            clock_rate: default_clock_rate(),
        }
    }
}

impl SourceSpec {
    /// Resolved profile parameters for the configured codec.
    ///
    /// `profile_idc`/`level_idc`/`tier_flag` are the numeric values passed
    /// to the native encoder and the H.265 SDP fmtp line; `profile_string`
    /// is the H.264-style profile-level-id (empty for H.265).
    fn resolve_profile(encoder: &EncoderSpec) -> anyhow::Result<(u32, u32, u32, String)> {
        let codec_str = encoder.codec.to_ascii_lowercase();
        let is_rkmpp = encoder.backend.eq_ignore_ascii_case("rkmpp");
        let is_cedar = encoder.backend.eq_ignore_ascii_case("cedar");

        if is_rkmpp && !matches!(codec_str.as_str(), "h264" | "h265" | "hevc") {
            anyhow::bail!(
                "encoder backend 'rkmpp' supports only h264 and h265, got '{}'",
                encoder.codec
            );
        }
        if is_cedar && !matches!(codec_str.as_str(), "h264" | "h265" | "hevc") {
            anyhow::bail!(
                "encoder backend 'cedar' supports only h264 and h265, got '{}'",
                encoder.codec
            );
        }

        // Resolve profile_idc / level_idc / tier_flag per codec.
        // Rust is the sole parsing layer; C++ receives numeric values directly.
        // rkmpp passes them to the encoder; cedar does too (the cedar VE
        // profile/level enums share the profile_idc/level_idc numbering), and
        // every backend needs the H.265 numerics for the SDP fmtp line.
        let (profile_idc, level_idc, tier_flag) = match codec_str.as_str() {
            "h264" if is_rkmpp || is_cedar => {
                let (pidc, lidc) = encoder.h264_profile_level_id()?;
                if is_cedar && pidc != 100 {
                    // The A733 cedar VE emits a stream whose slices do not match
                    // the SPS for Baseline/Main (decoders conceal to green/gray);
                    // only High profile produces a valid stream. Verified
                    // on-device across Baseline/Main/High × L3.1/L4.0/L5.1.
                    anyhow::bail!(
                        "encoder backend 'cedar' supports only H.264 High profile \
                         (e.g. profile = \"640028\"); got '{}' (profile_idc {})",
                        encoder.profile,
                        pidc
                    );
                }
                (u32::from(pidc), u32::from(lidc), 0u32)
            }
            "h265" | "hevc" => {
                // h265_profile_params returns (profile_idc, tier_flag, level_idc)
                let (pidc, tier, lidc) = encoder.h265_profile_params()?;
                (u32::from(pidc), u32::from(lidc), u32::from(tier))
            }
            // v4l2-m2m / rdk H.264 and the other codecs: profile/level are
            // resolved via string helpers; numeric fields are unused.
            _ => (0u32, 0u32, 0u32),
        };

        // H.265 SDP fmtp is built from the numeric fields above; the
        // H.264-style profile-level-id string does not apply.
        let is_h265 = matches!(codec_str.as_str(), "h265" | "hevc");
        let profile_string = if is_h265 {
            String::new()
        } else {
            encoder.profile_level_id()?
        };

        Ok((profile_idc, level_idc, tier_flag, profile_string))
    }
}

impl SourceSpec {
    /// Native capture feeds raw frames straight into the encoder, so
    /// compressed MJPEG is unsupported end-to-end in the livehal pipeline
    /// (the V4L2 backend rejects it at runtime too).  Surface it here, at
    /// configuration time, instead of failing on the device.
    fn ensure_supported_pixel_format(capture: &CaptureSpec) -> anyhow::Result<()> {
        if pixel_format_from_str(&capture.pixel_format)? == PixelFormat::Mjpeg {
            anyhow::bail!(
                "capture.pixel_format: MJPEG capture is not supported by \
                 the livehal pipeline; use yuyv, uyvy, nv12, yuv420 or \
                 rgb888"
            );
        }
        Ok(())
    }

    /// Validate the source specification.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.stream_id.trim().is_empty() {
            anyhow::bail!("stream_id cannot be empty");
        }

        // A native source is video-only (capture + encoder), audio-only
        // (audio_capture + audio_encoder), or video+audio.  Each leg must
        // be complete or absent entirely — half-legs are config errors.
        let video_present = self.capture.is_some() || self.encoder.is_some();
        let audio_present = self.audio_capture.is_some() || self.audio_encoder.is_some();
        if !video_present && !audio_present {
            anyhow::bail!(
                "a native source needs at least one complete media leg: \
                 capture + encoder (video) or audio_capture + audio_encoder (audio)"
            );
        }

        if video_present {
            let capture = self.capture.as_ref().ok_or_else(|| {
                anyhow::anyhow!("encoder requires a matching capture (incomplete video leg)")
            })?;
            let encoder = self.encoder.as_ref().ok_or_else(|| {
                anyhow::anyhow!("capture requires a matching encoder (incomplete video leg)")
            })?;
            Self::validate_video_leg(capture, encoder)?;
        }

        if audio_present {
            let audio_capture = self.audio_capture.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "audio_encoder requires a matching audio_capture (incomplete audio leg)"
                )
            })?;
            let audio_encoder = self.audio_encoder.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "audio_capture requires a matching audio_encoder (incomplete audio leg)"
                )
            })?;
            #[cfg(not(feature = "encoder-opus"))]
            {
                let _ = (audio_capture, audio_encoder);
                anyhow::bail!(
                    "audio sources (audio_capture/audio_encoder) require the 'encoder-opus' feature"
                );
            }
            #[cfg(feature = "encoder-opus")]
            Self::validate_audio_leg(audio_capture, audio_encoder)?;
        }

        let mut tier_names = std::collections::HashSet::new();
        for tier in &self.tiers {
            if tier.name.trim().is_empty() {
                anyhow::bail!("tiers.name cannot be empty");
            }
            if !tier_names.insert(tier.name.as_str()) {
                anyhow::bail!("duplicate tier name '{}'", tier.name);
            }
            if let Some(bps) = tier.encoder.bitrate {
                if bps == 0 {
                    anyhow::bail!("tier '{}': encoder.bitrate must be non-zero", tier.name);
                }
                // The only upper bound is the encoder control channel (a
                // signed 32-bit value): a rung may legitimately exceed the
                // boot encoder.bitrate — the ladder top need not be the boot
                // profile (2M boot with a 3M max-quality rung).
                if bps > i32::MAX as u32 {
                    anyhow::bail!(
                        "tier '{}': encoder.bitrate {} exceeds the encoder control range",
                        tier.name,
                        bps
                    );
                }
            }
            // Capture fields turn the tier into a ladder rung that
            // rebuilds the pipeline: dimensions must pair up and be
            // non-zero.  There is deliberately no bound against the
            // configured capture — a rung may exceed the boot profile
            // (e.g. a 972p30 boot with a 1080p20 max-quality rung) or
            // trade resolution for framerate (1080p30 base, 480p60
            // rung).  Sensor/ISP capability is enforced at apply time:
            // a mode the camera rejects rolls the pipeline back.
            match (tier.capture.width, tier.capture.height) {
                (Some(0), _) | (_, Some(0)) => {
                    anyhow::bail!(
                        "tier '{}': capture.width/height must be non-zero",
                        tier.name
                    );
                }
                (Some(_), Some(_)) | (None, None) => {}
                _ => {
                    anyhow::bail!(
                        "tier '{}': capture.width and capture.height must be set together",
                        tier.name
                    );
                }
            }
            if let Some(fps) = tier.capture.fps
                && fps == 0
            {
                anyhow::bail!("tier '{}': capture.fps must be non-zero", tier.name);
            }
            // Geometry rungs rebuild the video capture+encoder pipeline;
            // an audio-only source has neither, so they are meaningless
            // there (tier.capture.* semantics stay video-only).
            if !video_present && tier.has_geometry() {
                anyhow::bail!(
                    "tier '{}': capture.width/height/fps are invalid on an audio-only source",
                    tier.name
                );
            }
            // A tier is a parameter preset — it must change something.
            if tier.capture.width.is_none()
                && tier.capture.height.is_none()
                && tier.capture.fps.is_none()
                && tier.encoder.bitrate.is_none()
            {
                anyhow::bail!(
                    "tier '{}': empty tier — set at least one capture or encoder parameter",
                    tier.name
                );
            }
        }
        Ok(())
    }

    /// Video-leg checks (capture geometry/pixel format, encoder
    /// codec/profile/rate control).
    fn validate_video_leg(capture: &CaptureSpec, encoder: &EncoderSpec) -> anyhow::Result<()> {
        let backend = capture.backend.to_lowercase();
        if backend != "libcamera" && backend != "v4l2" && backend != "mock" {
            anyhow::bail!(
                "capture.backend must be 'libcamera', 'v4l2', or 'mock', got '{}'",
                capture.backend
            );
        }
        // The mock backend synthesizes frames and opens no device, so the
        // device field is neither required nor used there.
        if backend != "mock" && capture.device.as_deref().unwrap_or("").trim().is_empty() {
            anyhow::bail!("capture.device cannot be empty");
        }
        // The mock backend generates NV12 only; reject anything else at
        // configuration time rather than at source start.
        if backend == "mock" {
            let format = pixel_format_from_str(&capture.pixel_format)
                .map_err(|e| anyhow::anyhow!("capture.pixel_format: {}", e))?;
            if format != PixelFormat::Nv12 {
                anyhow::bail!(
                    "capture.pixel_format: the mock capture backend generates NV12 only, got '{}'",
                    capture.pixel_format
                );
            }
        }
        // capture.pattern selects the mock backend's background pattern and
        // is meaningless for real devices.
        if let Some(pattern) = &capture.pattern {
            if backend != "mock" {
                anyhow::bail!(
                    "capture.pattern is supported only by the mock capture backend, got '{}'",
                    capture.backend
                );
            }
            if !MOCK_CAPTURE_PATTERNS.contains(&pattern.to_lowercase().as_str()) {
                anyhow::bail!(
                    "capture.pattern: unknown mock pattern '{}'. Supported: {}",
                    pattern,
                    MOCK_CAPTURE_PATTERNS.join(", ")
                );
            }
        }
        if (capture.hflip || capture.vflip) && backend != "libcamera" {
            anyhow::bail!(
                "capture.hflip/vflip are supported only by the libcamera backend: \
                 the v4l2 video node (unicam) only DMA-moves frames and has no \
                 image-processing capability; flip controls exist only on the \
                 sensor sub-device, and setting them behind libcamera's back \
                 would desync the ISP's Bayer order"
            );
        }
        if capture.width == 0 || capture.height == 0 {
            anyhow::bail!("capture width/height must be non-zero");
        }
        if capture.fps == 0 {
            anyhow::bail!("capture.fps must be non-zero");
        }
        if encoder.bitrate == 0 {
            anyhow::bail!("encoder.bitrate must be non-zero");
        }
        // gop = 0 (no periodic keyframes; IDR only on request) is a
        // v4l2-m2m / bcm2835-codec capability — MMAL INTRAPERIOD = 0.
        // Other backends have no defined zero semantics, so keep
        // rejecting it there.
        if encoder.gop == 0 && !encoder.backend.eq_ignore_ascii_case("v4l2-m2m") {
            anyhow::bail!(
                "encoder.gop must be non-zero (gop = 0 is supported only by the v4l2-m2m backend)"
            );
        }
        if let Some(mode) = &encoder.bitrate_mode {
            bitrate_mode_to_u32(mode)
                .map_err(|e| anyhow::anyhow!("encoder.bitrate_mode: {}", e))?;
        }
        if let Some(AdaptiveConfig::Knobs {
            min_bitrate: Some(min),
        }) = &encoder.adaptive
            && *min > encoder.bitrate
        {
            anyhow::bail!("encoder.adaptive.min_bitrate must not exceed encoder.bitrate");
        }

        let encoder_backend = encoder.backend.to_lowercase();
        if encoder_backend != "v4l2-m2m"
            && encoder_backend != "rdk"
            && encoder_backend != "rkmpp"
            && encoder_backend != "cedar"
        {
            anyhow::bail!(
                "encoder.backend must be 'v4l2-m2m', 'rdk', 'rkmpp', or 'cedar', got '{}'",
                encoder.backend
            );
        }

        // Profile/level/tier are resolved with the same rules
        // to_native_params() uses, so anything validate() accepts can be
        // converted and vice versa.
        if let Err(e) = Self::resolve_profile(encoder) {
            anyhow::bail!("encoder.profile/level/tier: {}", e);
        }

        // Validate pixel_format and codec strings early so config errors
        // surface during validation rather than at source creation time.
        Self::ensure_supported_pixel_format(capture)?;
        pixel_format_to_u32(&capture.pixel_format)
            .map_err(|e| anyhow::anyhow!("capture.pixel_format: {}", e))?;
        codec_to_u32(&encoder.codec).map_err(|e| anyhow::anyhow!("encoder.codec: {}", e))?;
        Ok(())
    }

    /// Audio-leg checks: the fixed Opus wire contract plus the encoder and
    /// capture parameter ranges livehal accepts.
    #[cfg(feature = "encoder-opus")]
    fn validate_audio_leg(
        capture: &AudioCaptureSpec,
        encoder: &AudioEncoderSpec,
    ) -> anyhow::Result<()> {
        if !encoder.codec.eq_ignore_ascii_case("opus") {
            anyhow::bail!(
                "audio_encoder.codec must be 'opus', got '{}'",
                encoder.codec
            );
        }
        let backend = capture.backend.to_lowercase();
        if backend != "alsa" && backend != "sine" {
            anyhow::bail!(
                "audio_capture.backend must be 'alsa' or 'sine', got '{}'",
                capture.backend
            );
        }
        if capture.device.trim().is_empty() {
            anyhow::bail!("audio_capture.device cannot be empty");
        }
        if capture.sample_rate != 48_000 {
            anyhow::bail!(
                "audio_capture.sample_rate must be 48000 (v1 has no resampling), got {}",
                capture.sample_rate
            );
        }
        if capture.channels != 1 && capture.channels != 2 {
            anyhow::bail!(
                "audio_capture.channels must be 1 or 2, got {}",
                capture.channels
            );
        }
        if !(6_000..=510_000).contains(&encoder.bitrate) {
            anyhow::bail!(
                "audio_encoder.bitrate must be in 6000..=510000, got {}",
                encoder.bitrate
            );
        }
        if audio_application(&encoder.application).is_none() {
            anyhow::bail!(
                "audio_encoder.application must be 'audio', 'voip' or 'lowdelay', got '{}'",
                encoder.application
            );
        }
        if ![5, 10, 20, 40, 60].contains(&encoder.frame_ms) {
            anyhow::bail!(
                "audio_encoder.frame_ms must be one of 5/10/20/40/60, got {}",
                encoder.frame_ms
            );
        }
        if encoder.complexity > 10 {
            anyhow::bail!(
                "audio_encoder.complexity must be in 0..=10, got {}",
                encoder.complexity
            );
        }
        if let Some(AdaptiveConfig::Knobs {
            min_bitrate: Some(min),
        }) = &encoder.adaptive
            && *min > encoder.bitrate
        {
            anyhow::bail!(
                "audio_encoder.adaptive.min_bitrate must not exceed audio_encoder.bitrate"
            );
        }
        Ok(())
    }
}

/// Parse the Opus application mode string (`audio` | `voip` | `lowdelay`).
#[cfg(feature = "encoder-opus")]
fn audio_application(s: &str) -> Option<livehal::AudioApplication> {
    match s.to_ascii_lowercase().as_str() {
        "audio" => Some(livehal::AudioApplication::Audio),
        "voip" => Some(livehal::AudioApplication::Voip),
        "lowdelay" => Some(livehal::AudioApplication::LowDelay),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Structured → NativeSourceParams conversion
// ---------------------------------------------------------------------------

/// Raw pixel formats understood by the native capture backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PixelFormat {
    Yuyv422 = 0,
    Nv12 = 1,
    Yuv420p = 2,
    Mjpeg = 3,
    Rgb888 = 4,
    Uyvy422 = 5,
}

impl From<PixelFormat> for u32 {
    fn from(p: PixelFormat) -> Self {
        p as u32
    }
}

/// Video codecs understood by the native encoder backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VideoCodec {
    H264 = 100,
    H265 = 101,
    Av1 = 102,
    Vp8 = 103,
    Vp9 = 104,
}

impl From<VideoCodec> for u32 {
    fn from(c: VideoCodec) -> Self {
        c as u32
    }
}

impl TryFrom<u32> for VideoCodec {
    type Error = anyhow::Error;

    fn try_from(value: u32) -> anyhow::Result<Self> {
        match value {
            100 => Ok(VideoCodec::H264),
            101 => Ok(VideoCodec::H265),
            102 => Ok(VideoCodec::Av1),
            103 => Ok(VideoCodec::Vp8),
            104 => Ok(VideoCodec::Vp9),
            other => anyhow::bail!("unsupported video codec value: {}", other),
        }
    }
}

/// Map a pixel format string to its `RawPixelFormat` numeric value.
///
/// Used when converting structured `CaptureSpec` into `NativeSourceParams`.
pub fn pixel_format_to_u32(s: &str) -> anyhow::Result<u32> {
    pixel_format_from_str(s).map(Into::into)
}

/// Parse a pixel format string into a typed enum.
pub fn pixel_format_from_str(s: &str) -> anyhow::Result<PixelFormat> {
    match s.to_lowercase().as_str() {
        "yuyv" | "yuyv422" => Ok(PixelFormat::Yuyv422),
        "nv12" => Ok(PixelFormat::Nv12),
        "yuv420" | "yuv420p" => Ok(PixelFormat::Yuv420p),
        "mjpeg" => Ok(PixelFormat::Mjpeg), // kept for the FFI mapping; SourceSpec rejects it
        "rgb888" | "rgb" => Ok(PixelFormat::Rgb888),
        "uyvy" | "uyvy422" => Ok(PixelFormat::Uyvy422),
        other => anyhow::bail!(
            "unsupported pixel_format: '{}'. Supported: yuyv, nv12, yuv420, rgb888, uyvy",
            other
        ),
    }
}

/// Background patterns of the synthetic `mock` capture backend.
///
/// Mirrors the `kPatterns` table in `livehal/native-pipeline/mock_capture.cpp`
/// (the C++ side validates again at `init()`); the names follow gst
/// `videotestsrc` conventions. Keep the two lists in sync.
pub const MOCK_CAPTURE_PATTERNS: &[&str] = &[
    "smpte75",
    "snow",
    "black",
    "white",
    "checkers",
    "gradient",
    "zone-plate",
];

/// Map a codec string to its `VideoCodec` numeric value.
///
/// Used when converting structured `EncoderSpec` into `NativeSourceParams`.
pub fn codec_to_u32(s: &str) -> anyhow::Result<u32> {
    video_codec_from_str(s).map(Into::into)
}

/// Map a rate-control mode string to its FFI numeric value:
/// 1 = VBR, 2 = CBR (0 = driver default is produced by an absent key).
pub fn bitrate_mode_to_u32(s: &str) -> anyhow::Result<u32> {
    match s.to_lowercase().as_str() {
        "vbr" => Ok(1),
        "cbr" => Ok(2),
        other => anyhow::bail!("unsupported bitrate_mode: '{}'. Supported: vbr, cbr", other),
    }
}

/// Parse a codec string into a typed enum.
pub fn video_codec_from_str(s: &str) -> anyhow::Result<VideoCodec> {
    match s.to_lowercase().as_str() {
        "h264" => Ok(VideoCodec::H264),
        "h265" | "hevc" => Ok(VideoCodec::H265),
        "av1" => Ok(VideoCodec::Av1),
        "vp8" => Ok(VideoCodec::Vp8),
        "vp9" => Ok(VideoCodec::Vp9),
        other => anyhow::bail!(
            "unsupported codec: '{}'. Supported: h264, h265, av1, vp8, vp9",
            other
        ),
    }
}

impl EncoderSpec {
    /// Resolve the effective H.264 profile-level-id string.
    ///
    /// For H.264 the level is encoded in the 6-char profile-level-id hex.
    /// When `profile` is a profile name, the separate `level` field is
    /// required to build the hex string.
    pub fn profile_level_id(&self) -> anyhow::Result<String> {
        if self.profile.len() == 6 && self.profile.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(self.profile.clone());
        }
        let level_idc = match self.level.as_deref() {
            Some(l) => h264_level_to_idc(l)?,
            None => anyhow::bail!(
                "encoder.level is required when profile is a profile name ('{}')",
                self.profile
            ),
        };
        let (profile_idc, constraint) = h264_profile_to_idc(&self.profile)?;
        Ok(format!("{}{}{:02x}", profile_idc, constraint, level_idc))
    }

    /// Parse a 6-char H.264 profile-level-id hex string into (profile_idc, level_idc).
    ///
    /// Only accepts Baseline (66), Main (77), and High (100) profile_idc.
    /// Rejects a separate `level` field — the level comes from the hex string only.
    pub fn h264_profile_level_id(&self) -> anyhow::Result<(u8, u8)> {
        if self.level.is_some() {
            anyhow::bail!(
                "H.264 does not use a separate 'level' field; \
                 the level is encoded in the profile-level-id hex (e.g. '420028' = Baseline Level 4.0). \
                 Remove the 'level' field from this encoder config."
            );
        }
        let hex = self.profile.trim();
        if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            anyhow::bail!(
                "H.264 requires a 6-character profile-level-id hex string (e.g. '420028'), got '{}'",
                self.profile
            );
        }
        let profile_idc = u8::from_str_radix(&hex[0..2], 16)?;
        let level_idc = u8::from_str_radix(&hex[4..6], 16)?;

        match profile_idc {
            66 | 77 | 100 => {}
            other => anyhow::bail!(
                "unsupported H.264 profile_idc {} (from '{}'); \
                 supported: 66=Baseline, 77=Main, 100=High",
                other,
                self.profile
            ),
        }
        if level_idc == 0 || level_idc > 62 {
            anyhow::bail!(
                "invalid H.264 level_idc {} in '{}' (max 62 = Level 6.2)",
                level_idc,
                self.profile
            );
        }
        Ok((profile_idc, level_idc))
    }

    /// Resolve H.265 profile parameters.
    ///
    /// Returns `(profile_idc, tier_flag, level_idc)`.
    /// MVP accepts only Main profile + Main tier.
    pub fn h265_profile_params(&self) -> anyhow::Result<(u8, u8, u8)> {
        let profile = self.profile.trim();

        // Hex check BEFORE name match — produces targeted error
        if profile.len() == 6 && profile.chars().all(|c| c.is_ascii_hexdigit()) {
            anyhow::bail!(
                "H.265 does not use H.264-style profile-level-id '{}'; use profile='main'",
                profile
            );
        }

        let profile_idc = match profile.to_ascii_lowercase().as_str() {
            "" | "main" => 1u8,
            other => anyhow::bail!(
                "unsupported H.265 profile '{}'; only 'main' is supported",
                other
            ),
        };

        let tier_flag = match self.tier.as_deref() {
            None | Some("main") => 0u8,
            Some("high") => anyhow::bail!("H.265 high tier is not supported in MVP"),
            Some(other) => anyhow::bail!("unsupported H.265 tier '{}'", other),
        };

        let level_idc = match self.level.as_deref() {
            Some(l) => h265_level_to_idc(l)?,
            None => 120u8, // default Level 4.0
        };

        Ok((profile_idc, tier_flag, level_idc))
    }
}

/// Map H.265 level strings (e.g. "4.0") to level_idc per HEVC spec.
/// level_idc = level × 30 (e.g. 4.0 → 120).
fn h265_level_to_idc(level: &str) -> anyhow::Result<u8> {
    match level.trim() {
        "3.0" => Ok(90),
        "3.1" => Ok(93),
        "4.0" => Ok(120),
        "4.1" => Ok(123),
        "5.0" => Ok(150),
        "5.1" => Ok(153),
        other => anyhow::bail!(
            "unsupported H.265 level '{}'. Supported: 3.0, 3.1, 4.0, 4.1, 5.0, 5.1",
            other
        ),
    }
}

/// Map common H.264 profile names to (profile_idc, constraint_set0_flag byte).
fn h264_profile_to_idc(name: &str) -> anyhow::Result<(&'static str, &'static str)> {
    match name.to_ascii_lowercase().as_str() {
        "baseline" => Ok(("42", "00")),
        "constrained-baseline" => Ok(("42", "C0")),
        "main" => Ok(("4D", "00")),
        "constrained-main" => Ok(("4D", "C0")),
        "extended" => Ok(("58", "00")),
        "high" => Ok(("64", "00")),
        "constrained-high" => Ok(("64", "C0")),
        "high-10" => Ok(("6E", "00")),
        "high-422" => Ok(("7A", "00")),
        "high-444" => Ok(("F4", "00")),
        other => anyhow::bail!(
            "unsupported H.264 profile name '{}'. Supported: baseline, constrained-baseline, main, constrained-main, extended, high, constrained-high, high-10, high-422, high-444",
            other
        ),
    }
}

/// Map H.264 level strings (e.g. "3.1", "5") to level_idc.
fn h264_level_to_idc(level: &str) -> anyhow::Result<u8> {
    let normalized = level.trim();
    let idc = if let Some(dot) = normalized.find('.') {
        let major: u8 = normalized[..dot].parse()?;
        let minor: u8 = normalized[dot + 1..].parse()?;
        major * 10 + minor
    } else {
        normalized.parse::<u8>()? * 10
    };
    if idc == 0 || idc > 62 {
        anyhow::bail!("invalid H.264 level '{}'", level);
    }
    Ok(idc)
}

impl SourceSpec {
    /// Build `NativeSourceParams` directly from a structured `SourceSpec`.
    ///
    /// This is the direct path — no URL-based roundtrip.
    /// Returns `None` when the source has no video leg (audio-only);
    /// returns an error if `pixel_format` or `codec` strings are
    /// unrecognised.
    #[cfg(feature = "native-source")]
    pub fn to_native_params(&self) -> anyhow::Result<Option<NativeSourceParams>> {
        let (Some(capture), Some(encoder)) = (&self.capture, &self.encoder) else {
            return Ok(None);
        };

        let backend = capture.backend.to_lowercase();
        if backend != "libcamera" && backend != "v4l2" && backend != "mock" {
            anyhow::bail!("unsupported capture backend: {}", capture.backend);
        }
        let capture_device = capture.device.clone().unwrap_or_default();

        // Same early rejection as validate(): MJPEG cannot reach the
        // native capture/encoder pipeline.
        Self::ensure_supported_pixel_format(capture)?;

        // Profile resolution is shared with validate() — see resolve_profile().
        let (profile_idc, level_idc, tier_flag, profile_string) = Self::resolve_profile(encoder)?;

        // Validation compares backend names case-insensitively while the C++
        // factory matches them exactly — pass the canonical lowercase form
        // so e.g. `backend = "Mock"` cannot fail at the FFI boundary.
        let encoder_backend = encoder.backend.to_lowercase();

        Ok(Some(NativeSourceParams {
            capture_backend: backend,
            capture_device,
            width: capture.width,
            height: capture.height,
            fps: capture.fps,
            capture_pixel_format: pixel_format_to_u32(&capture.pixel_format)?,
            capture_pattern: capture
                .pattern
                .as_deref()
                .unwrap_or_default()
                .to_lowercase(),
            encoder_backend,
            codec: codec_to_u32(&encoder.codec)?,
            bitrate: encoder.bitrate,
            profile: profile_string.clone(),
            gop: encoder.gop,
            bitrate_mode: match &encoder.bitrate_mode {
                Some(mode) => bitrate_mode_to_u32(mode)?,
                None => 0,
            },
            payload_type: self.output.payload_type as u32,
            clock_rate: self.output.clock_rate,
            capture_prefer_dmabuf: capture.prefer_dmabuf as u8,
            capture_hflip: capture.hflip as u8,
            capture_vflip: capture.vflip as u8,
            encoder_prefer_dmabuf: encoder.prefer_dmabuf as u8,
            codec_name: encoder.codec.to_uppercase(),
            default_profile: profile_string,
            profile_idc,
            level_idc,
            tier_flag,
        }))
    }

    /// Build livehal audio pipeline params from the audio leg.
    ///
    /// Returns `None` when the source has no audio leg.  The mapping is
    /// infallible because [`SourceSpec::validate`] has already checked
    /// every field; the wire contract is fixed (Opus 48 kHz, RTP payload
    /// type 111, clock 48000, RFC 7587).
    #[cfg(feature = "encoder-opus")]
    pub fn to_audio_params(&self) -> Option<livehal::AudioSourceParams> {
        let capture = self.audio_capture.as_ref()?;
        let encoder = self.audio_encoder.as_ref()?;
        Some(livehal::AudioSourceParams {
            capture_backend: capture.backend.clone(),
            capture_device: capture.device.clone(),
            sample_rate: capture.sample_rate,
            channels: capture.channels,
            bitrate: encoder.bitrate,
            application: audio_application(&encoder.application)
                .unwrap_or(livehal::AudioApplication::Audio),
            frame_ms: encoder.frame_ms,
            vbr: encoder.vbr,
            inband_fec: encoder.inband_fec,
            dtx: encoder.dtx,
            payload_type: 111,
            clock_rate: 48_000,
            complexity: encoder.complexity,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn v4l2_spec() -> SourceSpec {
        SourceSpec {
            stream_id: "cam1".into(),
            capture: Some(CaptureSpec {
                backend: "v4l2".into(),
                device: Some("/dev/video0".into()),
                width: 640,
                height: 480,
                fps: 30,
                pixel_format: "yuyv".into(),
                prefer_dmabuf: false,
                hflip: false,
                vflip: false,
                pattern: None,
            }),
            encoder: Some(EncoderSpec {
                backend: "v4l2-m2m".into(),
                codec: "h264".into(),
                bitrate: 1_500_000,
                profile: "42001f".into(),
                level: None,
                tier: None,
                gop: 60,
                bitrate_mode: None,
                prefer_dmabuf: false,
                adaptive: None,
            }),
            audio_capture: None,
            audio_encoder: None,
            output: OutputSpec::default(),
            tiers: vec![],
        }
    }

    fn libcamera_spec() -> SourceSpec {
        SourceSpec {
            stream_id: "pi-cam".into(),
            capture: Some(CaptureSpec {
                backend: "libcamera".into(),
                device: Some("0".into()),
                width: 1920,
                height: 1080,
                fps: 30,
                pixel_format: "nv12".into(),
                prefer_dmabuf: true,
                hflip: false,
                vflip: false,
                pattern: None,
            }),
            encoder: Some(EncoderSpec {
                backend: "rdk".into(),
                codec: "h264".into(),
                bitrate: 2_000_000,
                profile: "high".into(),
                level: Some("4.2".into()),
                tier: None,
                gop: 60,
                bitrate_mode: None,
                prefer_dmabuf: false,
                adaptive: None,
            }),
            audio_capture: None,
            audio_encoder: None,
            output: OutputSpec::default(),
            tiers: vec![],
        }
    }

    fn rkmpp_spec() -> SourceSpec {
        SourceSpec {
            stream_id: "rk-cam".into(),
            capture: Some(CaptureSpec {
                backend: "v4l2".into(),
                device: Some("/dev/video11".into()),
                width: 1920,
                height: 1080,
                fps: 30,
                pixel_format: "nv12".into(),
                prefer_dmabuf: false,
                hflip: false,
                vflip: false,
                pattern: None,
            }),
            encoder: Some(EncoderSpec {
                backend: "rkmpp".into(),
                codec: "h264".into(),
                bitrate: 4_000_000,
                profile: "640028".into(), // High Level 4.0
                level: None,
                tier: None,
                gop: 60,
                bitrate_mode: None,
                prefer_dmabuf: false,
                adaptive: None,
            }),
            audio_capture: None,
            audio_encoder: None,
            output: OutputSpec::default(),
            tiers: vec![],
        }
    }

    fn cedar_spec() -> SourceSpec {
        SourceSpec {
            stream_id: "sunxi-cam".into(),
            capture: Some(CaptureSpec {
                backend: "v4l2".into(),
                device: Some("/dev/video0".into()),
                width: 640,
                height: 480,
                fps: 30,
                pixel_format: "nv12".into(), // negotiates NV12M (2-plane MPLANE)
                prefer_dmabuf: false,
                hflip: false,
                vflip: false,
                pattern: None,
            }),
            encoder: Some(EncoderSpec {
                backend: "cedar".into(),
                codec: "h264".into(),
                bitrate: 2_000_000,
                // The A733 cedar VE only produces a valid H.264 stream in
                // High profile (see resolve_profile guard); 640028 = High@4.0.
                profile: "640028".into(),
                level: None,
                tier: None,
                gop: 60,
                bitrate_mode: None,
                prefer_dmabuf: false,
                adaptive: None,
            }),
            audio_capture: None,
            audio_encoder: None,
            output: OutputSpec::default(),
            tiers: vec![],
        }
    }

    fn mock_spec() -> SourceSpec {
        SourceSpec {
            stream_id: "mock-test".into(),
            capture: Some(CaptureSpec {
                backend: "mock".into(),
                device: None, // synthetic source: no device needed
                width: 1280,
                height: 720,
                fps: 30,
                pixel_format: "nv12".into(),
                prefer_dmabuf: false,
                hflip: false,
                vflip: false,
                pattern: None,
            }),
            encoder: Some(EncoderSpec {
                backend: "v4l2-m2m".into(),
                codec: "h264".into(),
                bitrate: 2_000_000,
                profile: "42001f".into(),
                level: None,
                tier: None,
                gop: 60,
                bitrate_mode: None,
                prefer_dmabuf: false,
                adaptive: None,
            }),
            audio_capture: None,
            audio_encoder: None,
            output: OutputSpec::default(),
            tiers: vec![],
        }
    }

    fn rkmpp_h265_spec() -> SourceSpec {
        SourceSpec {
            stream_id: "rk-h265".into(),
            capture: Some(CaptureSpec {
                backend: "v4l2".into(),
                device: Some("/dev/video11".into()),
                width: 1920,
                height: 1080,
                fps: 30,
                pixel_format: "nv12".into(),
                prefer_dmabuf: false,
                hflip: false,
                vflip: false,
                pattern: None,
            }),
            encoder: Some(EncoderSpec {
                backend: "rkmpp".into(),
                codec: "h265".into(),
                bitrate: 4_000_000,
                profile: "main".into(),
                level: Some("4.0".into()),
                tier: None,
                gop: 60,
                bitrate_mode: None,
                prefer_dmabuf: false,
                adaptive: None,
            }),
            audio_capture: None,
            audio_encoder: None,
            output: OutputSpec::default(),
            tiers: vec![],
        }
    }

    fn audio_spec() -> SourceSpec {
        SourceSpec {
            stream_id: "mic1".into(),
            capture: None,
            encoder: None,
            audio_capture: Some(AudioCaptureSpec {
                backend: "sine".into(),
                device: "default".into(),
                sample_rate: 48_000,
                channels: 2,
            }),
            audio_encoder: Some(AudioEncoderSpec {
                codec: "opus".into(),
                bitrate: 64_000,
                application: "audio".into(),
                frame_ms: 20,
                vbr: true,
                inband_fec: false,
                dtx: false,
                complexity: 9,
                adaptive: None,
            }),
            output: OutputSpec::default(),
            tiers: vec![],
        }
    }

    // --- SourceSpec validation tests ---

    #[test]
    fn test_source_spec_validate_v4l2_ok() {
        assert!(v4l2_spec().validate().is_ok());
    }

    #[test]
    fn test_source_spec_validate_libcamera_ok() {
        assert!(libcamera_spec().validate().is_ok());
    }

    #[test]
    fn test_source_spec_validate_cedar_ok() {
        assert!(cedar_spec().validate().is_ok());
    }

    #[test]
    fn test_source_spec_validate_unknown_encoder_backend_rejected() {
        let mut spec = cedar_spec();
        spec.encoder.as_mut().unwrap().backend = "nonexistent".into();
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_source_spec_validate_empty_id() {
        let mut spec = v4l2_spec();
        spec.stream_id = "  ".into();
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_source_spec_validate_missing_v4l2_device() {
        let mut spec = v4l2_spec();
        spec.capture.as_mut().unwrap().device = None;
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_source_spec_validate_missing_libcamera_device() {
        let mut spec = libcamera_spec();
        spec.capture.as_mut().unwrap().device = None;
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_source_spec_validate_zero_size() {
        let mut spec = v4l2_spec();
        spec.capture.as_mut().unwrap().width = 0;
        spec.capture.as_mut().unwrap().height = 0;
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_source_spec_rejects_mjpeg() {
        let mut spec = v4l2_spec();
        spec.capture.as_mut().unwrap().pixel_format = "mjpeg".into();
        assert!(spec.validate().is_err());
    }

    // --- mock capture backend tests (issue #519) ---

    #[test]
    fn test_source_spec_validate_mock_ok() {
        // No device required: the mock backend synthesizes its frames.
        assert!(mock_spec().validate().is_ok());
        // A device string is accepted but ignored.
        let mut spec = mock_spec();
        spec.capture.as_mut().unwrap().device = Some("ignored".into());
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_source_spec_validate_mock_nv12_only() {
        for format in ["yuyv", "yuv420", "rgb888", "uyvy"] {
            let mut spec = mock_spec();
            spec.capture.as_mut().unwrap().pixel_format = format.into();
            let err = spec.validate().unwrap_err();
            assert!(
                err.to_string().contains("NV12 only"),
                "pixel_format '{format}' must be rejected for mock: {err}"
            );
        }
    }

    #[test]
    fn test_source_spec_validate_mock_rejects_unknown_backend_change() {
        // Guard the allowlist itself: an unknown backend must still fail
        // even when a device is present.
        let mut spec = mock_spec();
        spec.capture.as_mut().unwrap().backend = "synthetic".into();
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_source_spec_validate_mock_zero_fps() {
        let mut spec = mock_spec();
        spec.capture.as_mut().unwrap().fps = 0;
        assert!(spec.validate().is_err());
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_mock() {
        let params = mock_spec().to_native_params().unwrap().unwrap();
        assert_eq!(params.capture_backend, "mock");
        assert_eq!(params.capture_device, "");
        assert_eq!(params.width, 1280);
        assert_eq!(params.height, 720);
        assert_eq!(params.fps, 30);
        assert_eq!(params.capture_pixel_format, 1); // nv12
        assert_eq!(params.capture_pattern, ""); // unset: backend default
        assert_eq!(params.encoder_backend, "v4l2-m2m");
        assert_eq!(params.codec, 100); // h264
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_lowercases_backends() {
        // Validation is case-insensitive; the C++ factory is not.  The
        // params must carry the canonical lowercase form.
        let mut spec = mock_spec();
        spec.capture.as_mut().unwrap().backend = "Mock".into();
        spec.encoder.as_mut().unwrap().backend = "V4L2-M2M".into();
        let params = spec.to_native_params().unwrap().unwrap();
        assert_eq!(params.capture_backend, "mock");
        assert_eq!(params.encoder_backend, "v4l2-m2m");
    }

    #[test]
    fn test_source_spec_validate_mock_pattern() {
        // Unset (backend default) and every listed pattern pass, any case.
        assert!(mock_spec().validate().is_ok());
        for pattern in MOCK_CAPTURE_PATTERNS {
            let mut spec = mock_spec();
            spec.capture.as_mut().unwrap().pattern = Some(pattern.to_string());
            assert!(
                spec.validate().is_ok(),
                "pattern '{pattern}' must validate for mock"
            );
        }
        let mut spec = mock_spec();
        spec.capture.as_mut().unwrap().pattern = Some("SNOW".into());
        assert!(
            spec.validate().is_ok(),
            "pattern matching is case-insensitive"
        );

        let mut spec = mock_spec();
        spec.capture.as_mut().unwrap().pattern = Some("bogus".into());
        let err = spec.validate().unwrap_err();
        assert!(
            err.to_string().contains("unknown mock pattern"),
            "unknown pattern must be rejected: {err}"
        );
    }

    #[test]
    fn test_source_spec_validate_pattern_mock_only() {
        // A pattern on a real capture device is a config error.
        let mut spec = v4l2_spec();
        spec.capture.as_mut().unwrap().pattern = Some("snow".into());
        let err = spec.validate().unwrap_err();
        assert!(
            err.to_string().contains("only by the mock capture backend"),
            "pattern must be rejected for v4l2: {err}"
        );
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_mock_pattern() {
        let mut spec = mock_spec();
        spec.capture.as_mut().unwrap().pattern = Some("Snow".into());
        let params = spec.to_native_params().unwrap().unwrap();
        assert_eq!(params.capture_pattern, "snow");
    }

    // --- capture flip (hflip/vflip) tests ---

    #[test]
    fn test_source_spec_validate_libcamera_flip_ok() {
        let mut spec = libcamera_spec();
        spec.capture.as_mut().unwrap().hflip = true;
        assert!(spec.validate().is_ok());
        spec.capture.as_mut().unwrap().vflip = true;
        assert!(spec.validate().is_ok()); // both = 180° rotation
    }

    #[test]
    fn test_source_spec_validate_v4l2_flip_rejected() {
        let mut spec = v4l2_spec();
        spec.capture.as_mut().unwrap().hflip = true;
        assert!(spec.validate().is_err());
        spec.capture.as_mut().unwrap().hflip = false;
        spec.capture.as_mut().unwrap().vflip = true;
        assert!(spec.validate().is_err());
    }

    // --- tier validation tests ---

    fn tier(name: &str, bitrate: u32) -> TierSpec {
        TierSpec {
            name: name.into(),
            capture: TierCaptureSpec::default(),
            encoder: TierEncoderSpec {
                bitrate: Some(bitrate),
            },
        }
    }

    #[test]
    fn test_tiers_valid() {
        let mut spec = rkmpp_spec();
        spec.tiers = vec![tier("low", 600_000), tier("mid", 2_000_000)];
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_tier_empty_name_rejected() {
        let mut spec = rkmpp_spec();
        spec.tiers = vec![tier("  ", 600_000)];
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_tier_duplicate_name_rejected() {
        let mut spec = rkmpp_spec();
        spec.tiers = vec![tier("low", 600_000), tier("low", 800_000)];
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_tier_duplicate_bitrate_allowed() {
        // Same bitrate on two rungs is fine — the active tier is tracked
        // by name, not matched by value.
        let mut spec = rkmpp_spec();
        spec.tiers = vec![tier("low", 600_000), tier("mid", 600_000)];
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_tier_bitrate_optional_with_geometry() {
        let mut spec = rkmpp_spec();
        let mut t = tier("low", 600_000);
        t.encoder.bitrate = None;
        t.capture.width = Some(640);
        t.capture.height = Some(480);
        spec.tiers = vec![t];
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_tier_empty_rejected() {
        // A tier must change at least one parameter.
        let mut spec = rkmpp_spec();
        let mut t = tier("bare", 600_000);
        t.encoder.bitrate = None;
        spec.tiers = vec![t];
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_tier_derived_bitrate() {
        // ~0.07 bpp, floored at 50 kbps.
        assert_eq!(TierSpec::derived_bitrate(640, 480, 30), 645_120);
        assert_eq!(TierSpec::derived_bitrate(1920, 1080, 20), 2_903_040);
        assert_eq!(TierSpec::derived_bitrate(320, 240, 10), 53_760);
        assert_eq!(TierSpec::derived_bitrate(1, 1, 1), 50_000);
    }

    #[test]
    fn test_tier_zero_bitrate_rejected() {
        let mut spec = rkmpp_spec();
        spec.tiers = vec![tier("low", 0)];
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_tier_bitrate_above_boot_allowed() {
        // A rung may exceed the boot encoder.bitrate (2M boot + 3M max
        // rung); the only bound is the encoder's signed-32-bit control.
        let mut spec = rkmpp_spec(); // encoder.bitrate = 4 Mbps
        spec.tiers = vec![tier("ultra", 5_000_000)];
        assert!(spec.validate().is_ok());
        spec.tiers = vec![tier("overflow", i32::MAX as u32 + 1)];
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_tier_resolution_downscale_ok() {
        let mut spec = rkmpp_spec(); // capture 1920x1080@30
        let mut t = tier("low", 600_000);
        t.capture.width = Some(1280);
        t.capture.height = Some(720);
        t.capture.fps = Some(15);
        spec.tiers = vec![t];
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_tier_width_without_height_rejected() {
        let mut spec = rkmpp_spec();
        let mut t = tier("low", 600_000);
        t.capture.width = Some(1280);
        spec.tiers = vec![t];
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_tier_zero_dimension_rejected() {
        let mut spec = rkmpp_spec();
        let mut t = tier("low", 600_000);
        t.capture.width = Some(0);
        t.capture.height = Some(720);
        spec.tiers = vec![t];
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_tier_above_boot_profile_allowed() {
        // A rung may exceed the boot capture profile: the ladder top need
        // not be the boot config (972p30 boot + 1080p20 maxres rung).
        let mut spec = rkmpp_spec(); // capture 1920x1080
        let mut t = tier("big", 600_000);
        t.capture.width = Some(2592);
        t.capture.height = Some(1944);
        spec.tiers = vec![t];
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_tier_fps_above_capture_allowed() {
        // A low-latency rung trades resolution for framerate: 1080p30
        // base with a 480p60 tier is a valid ladder on e.g. the OV5647.
        let mut spec = rkmpp_spec(); // capture 1920x1080@30
        let mut t = tier("lowlatency", 600_000);
        t.capture.width = Some(640);
        t.capture.height = Some(480);
        t.capture.fps = Some(60);
        spec.tiers = vec![t];
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_tier_zero_fps_rejected() {
        let mut spec = rkmpp_spec();
        let mut t = tier("low", 600_000);
        t.capture.fps = Some(0);
        spec.tiers = vec![t];
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_adaptive_key_forms() {
        let mut spec = rkmpp_spec(); // encoder.bitrate = 4 Mbps
        // Off is valid.
        spec.encoder.as_mut().unwrap().adaptive = Some(AdaptiveConfig::Switch(false));
        assert!(spec.validate().is_ok());
        // A floor above the ceiling is rejected.
        spec.encoder.as_mut().unwrap().adaptive = Some(AdaptiveConfig::Knobs {
            min_bitrate: Some(5_000_000),
        });
        assert!(spec.validate().is_err());
        // A sane custom floor is fine.
        spec.encoder.as_mut().unwrap().adaptive = Some(AdaptiveConfig::Knobs {
            min_bitrate: Some(300_000),
        });
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_adaptive_toml_forms() {
        // Untagged key: bool switch, inline knobs, and absent all parse.
        // The fragments are the contents of a `[encoder]` table, so the
        // wrapper struct expects that table header.
        #[derive(Deserialize)]
        struct W {
            encoder: EncoderSpec,
        }
        let w: W = toml::from_str(
            r#"
            [encoder]
            backend = "rkmpp"
            codec = "h264"
            bitrate = 4000000
            profile = "640028"
            adaptive = false
            "#,
        )
        .unwrap();
        assert!(matches!(
            w.encoder.adaptive,
            Some(AdaptiveConfig::Switch(false))
        ));

        let w: W = toml::from_str(
            r#"
            [encoder]
            backend = "rkmpp"
            codec = "h264"
            bitrate = 4000000
            profile = "640028"
            adaptive = { min_bitrate = 150000 }
            "#,
        )
        .unwrap();
        assert!(matches!(
            w.encoder.adaptive,
            Some(AdaptiveConfig::Knobs {
                min_bitrate: Some(150_000)
            })
        ));

        let w: W = toml::from_str(
            r#"
            [encoder]
            backend = "rkmpp"
            codec = "h264"
            bitrate = 4000000
            profile = "640028"
            "#,
        )
        .unwrap();
        assert!(w.encoder.adaptive.is_none());
    }

    // --- pixel_format / codec mapping tests ---

    #[test]
    fn test_pixel_format_to_u32_valid() {
        assert_eq!(pixel_format_to_u32("yuyv").unwrap(), 0);
        assert_eq!(pixel_format_to_u32("YUYV422").unwrap(), 0);
        assert_eq!(pixel_format_to_u32("nv12").unwrap(), 1);
        assert_eq!(pixel_format_to_u32("yuv420").unwrap(), 2);
        assert_eq!(pixel_format_to_u32("yuv420p").unwrap(), 2);
        assert_eq!(pixel_format_to_u32("mjpeg").unwrap(), 3);
        assert_eq!(pixel_format_to_u32("rgb888").unwrap(), 4);
        assert_eq!(pixel_format_to_u32("rgb").unwrap(), 4);
        assert_eq!(pixel_format_to_u32("uyvy").unwrap(), 5);
        assert_eq!(pixel_format_to_u32("UYVY422").unwrap(), 5);
    }

    #[test]
    fn test_pixel_format_to_u32_invalid() {
        assert!(pixel_format_to_u32("yyyv").is_err());
        assert!(pixel_format_to_u32("").is_err());
        assert!(pixel_format_to_u32("h264").is_err());
    }

    #[test]
    fn test_codec_to_u32_valid() {
        assert_eq!(codec_to_u32("h264").unwrap(), 100);
        assert_eq!(codec_to_u32("H264").unwrap(), 100);
        assert_eq!(codec_to_u32("h265").unwrap(), 101);
        assert_eq!(codec_to_u32("hevc").unwrap(), 101);
        assert_eq!(codec_to_u32("av1").unwrap(), 102);
        assert_eq!(codec_to_u32("vp8").unwrap(), 103);
        assert_eq!(codec_to_u32("vp9").unwrap(), 104);
    }

    #[test]
    fn test_codec_to_u32_invalid() {
        assert!(codec_to_u32("h266").is_err());
        assert!(codec_to_u32("").is_err());
        assert!(codec_to_u32("mjpeg").is_err());
    }

    #[test]
    fn test_bitrate_mode_to_u32() {
        assert_eq!(bitrate_mode_to_u32("vbr").unwrap(), 1);
        assert_eq!(bitrate_mode_to_u32("CBR").unwrap(), 2);
        assert!(bitrate_mode_to_u32("auto").is_err());
    }

    #[test]
    fn test_bitrate_mode_validation() {
        let mut spec = v4l2_spec();
        spec.encoder.as_mut().unwrap().bitrate_mode = Some("vbr".into());
        assert!(spec.validate().is_ok());
        spec.encoder.as_mut().unwrap().bitrate_mode = Some("auto".into());
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_gop_zero_only_v4l2_m2m() {
        // gop = 0 (IDR only on request) is a v4l2-m2m capability.
        let mut spec = v4l2_spec();
        spec.encoder.as_mut().unwrap().gop = 0;
        assert!(spec.validate().is_ok());

        let mut spec = rkmpp_spec();
        spec.encoder.as_mut().unwrap().gop = 0;
        assert!(spec.validate().is_err());
    }

    // --- profile/level/tier tests ---

    #[test]
    fn test_profile_level_id_hex_passthrough() {
        let enc = EncoderSpec {
            profile: "420028".into(),
            level: None,
            ..Default::default()
        };
        assert_eq!(enc.profile_level_id().unwrap(), "420028");
    }

    #[test]
    fn test_profile_level_id_from_name() {
        let enc = EncoderSpec {
            profile: "high".into(),
            level: Some("4.2".into()),
            ..Default::default()
        };
        assert_eq!(enc.profile_level_id().unwrap(), "64002a");
    }

    #[test]
    fn test_h264_level_6_2_maps_to_62() {
        assert_eq!(h264_level_to_idc("6.2").unwrap(), 62);
    }

    #[test]
    fn test_h264_level_above_6_2_rejected() {
        assert!(h264_level_to_idc("6.3").is_err());
        assert!(h264_level_to_idc("18.6").is_err());
    }

    #[test]
    fn test_profile_level_id_name_without_level() {
        let enc = EncoderSpec {
            profile: "main".into(),
            level: None,
            ..Default::default()
        };
        assert!(enc.profile_level_id().is_err());
    }

    // --- H.264 profile-level-id numeric tests ---

    #[test]
    fn test_h264_profile_420028() {
        let enc = EncoderSpec {
            profile: "420028".into(),
            codec: "h264".into(),
            level: None,
            ..Default::default()
        };
        let (pidc, lidc) = enc.h264_profile_level_id().unwrap();
        assert_eq!(pidc, 66); // Baseline
        assert_eq!(lidc, 40); // Level 4.0
    }

    #[test]
    fn test_h264_profile_4d0028() {
        let enc = EncoderSpec {
            profile: "4d0028".into(),
            codec: "h264".into(),
            level: None,
            ..Default::default()
        };
        let (pidc, _) = enc.h264_profile_level_id().unwrap();
        assert_eq!(pidc, 77); // Main
    }

    #[test]
    fn test_h264_profile_640028() {
        let enc = EncoderSpec {
            profile: "640028".into(),
            codec: "h264".into(),
            level: None,
            ..Default::default()
        };
        let (pidc, _) = enc.h264_profile_level_id().unwrap();
        assert_eq!(pidc, 100); // High
    }

    #[test]
    fn test_h264_with_separate_level_rejected() {
        let enc = EncoderSpec {
            profile: "640028".into(),
            codec: "h264".into(),
            level: Some("4.2".into()),
            ..Default::default()
        };
        assert!(enc.h264_profile_level_id().is_err());
    }

    #[test]
    fn test_h264_unsupported_profile_rejected() {
        let enc = EncoderSpec {
            profile: "580028".into(), // Extended
            codec: "h264".into(),
            level: None,
            ..Default::default()
        };
        assert!(enc.h264_profile_level_id().is_err());
    }

    // --- H.265 profile params tests ---

    #[test]
    fn test_h265_profile_main_valid() {
        let enc = EncoderSpec {
            profile: "main".into(),
            codec: "h265".into(),
            level: Some("4.0".into()),
            tier: None,
            ..Default::default()
        };
        let (pidc, tier, lidc) = enc.h265_profile_params().unwrap();
        assert_eq!(pidc, 1);
        assert_eq!(tier, 0);
        assert_eq!(lidc, 120);
    }

    #[test]
    fn test_h265_profile_empty_defaults() {
        let enc = EncoderSpec {
            profile: "".into(),
            codec: "h265".into(),
            level: None,
            tier: None,
            ..Default::default()
        };
        let (pidc, tier, lidc) = enc.h265_profile_params().unwrap();
        assert_eq!(pidc, 1);
        assert_eq!(tier, 0);
        assert_eq!(lidc, 120);
    }

    #[test]
    fn test_h265_hex_profile_rejected() {
        let enc = EncoderSpec {
            profile: "42001f".into(),
            codec: "h265".into(),
            level: Some("4.0".into()),
            tier: None,
            ..Default::default()
        };
        assert!(enc.h265_profile_params().is_err());
    }

    #[test]
    fn test_h265_bad_profile_rejected() {
        let enc = EncoderSpec {
            profile: "foo".into(),
            codec: "h265".into(),
            level: Some("4.0".into()),
            tier: None,
            ..Default::default()
        };
        assert!(enc.h265_profile_params().is_err());
    }

    #[test]
    fn test_h265_high_tier_rejected() {
        let enc = EncoderSpec {
            profile: "main".into(),
            codec: "h265".into(),
            level: Some("4.0".into()),
            tier: Some("high".into()),
            ..Default::default()
        };
        assert!(enc.h265_profile_params().is_err());
    }

    #[test]
    fn test_h265_level_4_0_maps_to_120() {
        assert_eq!(h265_level_to_idc("4.0").unwrap(), 120);
    }

    #[test]
    fn test_h265_level_4_1_maps_to_123() {
        assert_eq!(h265_level_to_idc("4.1").unwrap(), 123);
    }

    #[test]
    fn test_h265_level_unsupported() {
        assert!(h265_level_to_idc("6.0").is_err());
    }

    // --- SourceSpec validation tests (rkmpp codec gate) ---

    #[test]
    fn test_rkmpp_h264_valid() {
        let spec = rkmpp_spec();
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_rkmpp_h265_valid() {
        let spec = rkmpp_h265_spec();
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_rkmpp_invalid_codec_av1() {
        let mut spec = rkmpp_h265_spec();
        spec.encoder.as_mut().unwrap().codec = "av1".into();
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_rkmpp_invalid_codec_vp8() {
        let mut spec = rkmpp_h265_spec();
        spec.encoder.as_mut().unwrap().codec = "vp8".into();
        assert!(spec.validate().is_err());
    }

    // --- validate() / to_native_params() consistency tests ---
    // Both use resolve_profile(), so they must accept and reject the same
    // profile/level combinations for every backend, not just rkmpp.

    #[test]
    fn test_validate_v4l2_h265_name_profile_without_level() {
        // H.265 profile names do not require a separate level; validate()
        // must accept what to_native_params() accepts.
        let mut spec = v4l2_spec();
        spec.encoder.as_mut().unwrap().codec = "h265".into();
        spec.encoder.as_mut().unwrap().profile = "main".into();
        spec.encoder.as_mut().unwrap().level = None;
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_validate_v4l2_h265_hex_profile_rejected() {
        // H.264-style hex profile-level-id is invalid for H.265 on every
        // backend — to_native_params() rejects it, so validate() must too.
        let mut spec = v4l2_spec();
        spec.encoder.as_mut().unwrap().codec = "h265".into();
        spec.encoder.as_mut().unwrap().profile = "42001f".into();
        spec.encoder.as_mut().unwrap().level = None;
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_validate_v4l2_h265_h264_profile_name_rejected() {
        // 'high' is an H.264 profile name; it must not pass H.265 validation.
        let mut spec = v4l2_spec();
        spec.encoder.as_mut().unwrap().codec = "h265".into();
        spec.encoder.as_mut().unwrap().profile = "high".into();
        spec.encoder.as_mut().unwrap().level = Some("4.0".into());
        assert!(spec.validate().is_err());
    }

    // --- NativeSourceParams conversion tests ---

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_v4l2() {
        let params = v4l2_spec().to_native_params().unwrap().unwrap();
        assert_eq!(params.capture_backend, "v4l2");
        assert_eq!(params.capture_device, "/dev/video0");
        assert_eq!(params.width, 640);
        assert_eq!(params.height, 480);
        assert_eq!(params.capture_pixel_format, 0); // yuyv
        assert_eq!(params.encoder_backend, "v4l2-m2m");
        assert_eq!(params.codec, 100); // h264
        assert_eq!(params.profile, "42001f");
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_cedar() {
        let params = cedar_spec().to_native_params().unwrap().unwrap();
        assert_eq!(params.capture_backend, "v4l2");
        assert_eq!(params.capture_device, "/dev/video0");
        assert_eq!(params.capture_pixel_format, 1); // nv12
        assert_eq!(params.encoder_backend, "cedar");
        assert_eq!(params.codec, 100); // h264
        assert_eq!(params.profile, "640028");
        // cedar parses the H.264 profile-level-id into numerics (like rkmpp):
        // the encoder must emit the profile the SDP advertises.
        assert_eq!(params.profile_idc, 100); // High
        assert_eq!(params.level_idc, 40); // Level 4.0
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_cedar_h264_rejects_non_high_profile() {
        // The cedar VE only produces a valid H.264 stream in High profile.
        let mut spec = cedar_spec();
        spec.encoder.as_mut().unwrap().profile = "42001f".into(); // Baseline
        assert!(spec.validate().is_err());
        let mut spec = cedar_spec();
        spec.encoder.as_mut().unwrap().profile = "4d0028".into(); // Main
        assert!(spec.validate().is_err());
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_cedar_h265() {
        let mut spec = cedar_spec();
        spec.encoder.as_mut().unwrap().codec = "h265".into();
        spec.encoder.as_mut().unwrap().profile = "main".into();
        spec.encoder.as_mut().unwrap().level = Some("4.0".into());
        let params = spec.to_native_params().unwrap().unwrap();
        assert_eq!(params.encoder_backend, "cedar");
        assert_eq!(params.codec, 101); // h265
        // H.265 numerics feed both the cedar encoder and the SDP fmtp line.
        assert_eq!(params.profile_idc, 1); // Main
        assert_eq!(params.level_idc, 120); // Level 4.0
        assert_eq!(params.tier_flag, 0);
        assert!(params.profile.is_empty()); // no H.264-style profile-level-id
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_cedar_rejects_unsupported_codec() {
        let mut spec = cedar_spec();
        spec.encoder.as_mut().unwrap().codec = "vp9".into();
        assert!(spec.validate().is_err());
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_libcamera() {
        let params = libcamera_spec().to_native_params().unwrap().unwrap();
        assert_eq!(params.capture_backend, "libcamera");
        assert_eq!(params.capture_device, "0");
        assert_eq!(params.encoder_backend, "rdk");
        assert_eq!(params.profile, "64002a");
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_capture_flip() {
        let mut spec = libcamera_spec();
        spec.capture.as_mut().unwrap().hflip = true;
        spec.capture.as_mut().unwrap().vflip = true;
        let params = spec.to_native_params().unwrap().unwrap();
        assert_eq!(params.capture_hflip, 1);
        assert_eq!(params.capture_vflip, 1);

        let params = v4l2_spec().to_native_params().unwrap().unwrap();
        assert_eq!(params.capture_hflip, 0);
        assert_eq!(params.capture_vflip, 0);
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_rkmpp_h264() {
        let params = rkmpp_spec().to_native_params().unwrap().unwrap();
        assert_eq!(params.capture_backend, "v4l2");
        assert_eq!(params.encoder_backend, "rkmpp");
        assert_eq!(params.capture_pixel_format, 1); // nv12
        assert_eq!(params.codec, 100); // h264
        assert_eq!(params.profile, "640028");
        assert_eq!(params.profile_idc, 100);
        assert_eq!(params.level_idc, 40);
        assert_eq!(params.tier_flag, 0);
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_rkmpp_h265() {
        let params = rkmpp_h265_spec().to_native_params().unwrap().unwrap();
        assert_eq!(params.capture_backend, "v4l2");
        assert_eq!(params.encoder_backend, "rkmpp");
        assert_eq!(params.capture_pixel_format, 1); // nv12
        assert_eq!(params.codec, 101); // h265
        assert_eq!(params.profile_idc, 1);
        assert_eq!(params.level_idc, 120);
        assert_eq!(params.tier_flag, 0);
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_rkmpp_h265_default_level() {
        // H.265 without an explicit level must pass validate() AND
        // to_native_params(), defaulting to Level 4.0 (120).
        let mut spec = rkmpp_h265_spec();
        spec.encoder.as_mut().unwrap().level = None;
        spec.validate().unwrap();
        let params = spec.to_native_params().unwrap().unwrap();
        assert_eq!(params.profile_idc, 1);
        assert_eq!(params.level_idc, 120);
        assert_eq!(params.tier_flag, 0);
        // H.265 does not use the H.264-style profile-level-id string.
        assert_eq!(params.default_profile, "");
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_v4l2_h265_numerics() {
        // Non-rkmpp backends also need resolved H.265 numerics for the SDP
        // fmtp line — they must not fall back to 0/0/0.
        let mut spec = v4l2_spec();
        spec.encoder.as_mut().unwrap().codec = "h265".into();
        spec.encoder.as_mut().unwrap().profile = "main".into();
        spec.encoder.as_mut().unwrap().level = None;
        let params = spec.to_native_params().unwrap().unwrap();
        assert_eq!(params.codec, 101); // h265
        assert_eq!(params.profile_idc, 1);
        assert_eq!(params.level_idc, 120);
        assert_eq!(params.tier_flag, 0);
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_invalid_pixel_format() {
        let mut spec = v4l2_spec();
        spec.capture.as_mut().unwrap().pixel_format = "bad_format".into();
        assert!(spec.to_native_params().is_err());
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_invalid_codec() {
        let mut spec = v4l2_spec();
        spec.encoder.as_mut().unwrap().codec = "h266".into();
        assert!(spec.to_native_params().is_err());
    }

    // --- audio leg tests ---

    #[test]
    fn test_source_spec_no_legs_rejected() {
        // Neither a video leg nor an audio leg: nothing to publish.
        let mut spec = v4l2_spec();
        spec.capture = None;
        spec.encoder = None;
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_source_spec_half_legs_rejected() {
        // Video half-legs.
        let mut spec = v4l2_spec();
        spec.encoder = None;
        assert!(spec.validate().is_err());
        let mut spec = v4l2_spec();
        spec.capture = None;
        assert!(spec.validate().is_err());
        // Audio half-legs (checked before the encoder-opus feature gate).
        let mut spec = audio_spec();
        spec.audio_encoder = None;
        assert!(spec.validate().is_err());
        let mut spec = audio_spec();
        spec.audio_capture = None;
        assert!(spec.validate().is_err());
    }

    #[test]
    #[cfg(not(feature = "encoder-opus"))]
    fn test_audio_leg_requires_encoder_opus_feature() {
        let err = audio_spec().validate().unwrap_err();
        assert!(
            err.to_string().contains("encoder-opus"),
            "unexpected error: {err}"
        );
    }

    #[test]
    #[cfg(feature = "encoder-opus")]
    fn test_audio_only_spec_validate_ok() {
        assert!(audio_spec().validate().is_ok());
    }

    #[test]
    #[cfg(feature = "encoder-opus")]
    fn test_video_audio_spec_validate_ok() {
        let mut spec = v4l2_spec();
        let audio = audio_spec();
        spec.audio_capture = audio.audio_capture;
        spec.audio_encoder = audio.audio_encoder;
        assert!(spec.validate().is_ok());
        assert!(spec.to_native_params().unwrap().is_some());
        assert!(spec.to_audio_params().is_some());
    }

    #[test]
    #[cfg(feature = "encoder-opus")]
    fn test_audio_bad_params_rejected() {
        let bad = |f: fn(&mut SourceSpec)| {
            let mut spec = audio_spec();
            f(&mut spec);
            spec.validate().is_err()
        };
        assert!(bad(
            |s| s.audio_capture.as_mut().unwrap().sample_rate = 44_100
        ));
        assert!(bad(|s| s.audio_capture.as_mut().unwrap().channels = 3));
        assert!(bad(|s| s.audio_encoder.as_mut().unwrap().frame_ms = 15));
        assert!(bad(|s| s.audio_encoder.as_mut().unwrap().bitrate = 5_000));
        assert!(bad(|s| s.audio_encoder.as_mut().unwrap().bitrate = 510_001));
        assert!(bad(|s| s.audio_encoder.as_mut().unwrap().complexity = 11));
        assert!(bad(
            |s| s.audio_encoder.as_mut().unwrap().codec = "aac".into()
        ));
        assert!(bad(
            |s| s.audio_capture.as_mut().unwrap().backend = "pulse".into()
        ));
        assert!(bad(
            |s| s.audio_encoder.as_mut().unwrap().application = "music".into()
        ));
        assert!(bad(
            |s| s.audio_capture.as_mut().unwrap().device = "  ".into()
        ));
    }

    #[test]
    #[cfg(feature = "encoder-opus")]
    fn test_audio_adaptive_min_bitrate_check() {
        let mut spec = audio_spec();
        spec.audio_encoder.as_mut().unwrap().adaptive = Some(AdaptiveConfig::Knobs {
            min_bitrate: Some(100_000),
        });
        assert!(spec.validate().is_err());
        spec.audio_encoder.as_mut().unwrap().adaptive = Some(AdaptiveConfig::Knobs {
            min_bitrate: Some(16_000),
        });
        assert!(spec.validate().is_ok());
        spec.audio_encoder.as_mut().unwrap().adaptive = Some(AdaptiveConfig::Switch(false));
        assert!(spec.validate().is_ok());
    }

    #[test]
    #[cfg(feature = "encoder-opus")]
    fn test_audio_only_tier_geometry_rejected() {
        // Geometry rungs are video-only; an audio-only source rejects them.
        let mut spec = audio_spec();
        let mut t = tier("low", 32_000);
        t.capture.width = Some(640);
        t.capture.height = Some(480);
        spec.tiers = vec![t];
        assert!(spec.validate().is_err());
        // Bitrate-only rungs target the audio encoder and are fine.
        spec.tiers = vec![tier("low", 32_000)];
        assert!(spec.validate().is_ok());
    }

    #[test]
    #[cfg(feature = "encoder-opus")]
    fn test_to_audio_params_mapping() {
        let params = audio_spec().to_audio_params().unwrap();
        assert_eq!(params.capture_backend, "sine");
        assert_eq!(params.capture_device, "default");
        assert_eq!(params.sample_rate, 48_000);
        assert_eq!(params.channels, 2);
        assert_eq!(params.bitrate, 64_000);
        assert!(matches!(
            params.application,
            livehal::AudioApplication::Audio
        ));
        assert_eq!(params.frame_ms, 20);
        assert!(params.vbr);
        assert!(!params.inband_fec);
        assert!(!params.dtx);
        assert_eq!(params.payload_type, 111);
        assert_eq!(params.clock_rate, 48_000);
        assert_eq!(params.complexity, 9);

        // Application string mapping.
        for (s, expected) in [
            ("audio", livehal::AudioApplication::Audio),
            ("voip", livehal::AudioApplication::Voip),
            ("lowdelay", livehal::AudioApplication::LowDelay),
            ("LOWDELAY", livehal::AudioApplication::LowDelay),
        ] {
            let mut spec = audio_spec();
            spec.audio_encoder.as_mut().unwrap().application = s.into();
            let params = spec.to_audio_params().unwrap();
            assert!(
                params.application == expected,
                "application '{s}' mapped wrongly"
            );
        }
    }

    #[test]
    #[cfg(feature = "encoder-opus")]
    fn test_audio_minimal_toml_form() {
        // The minimal audio encoder form is `codec = "opus"` alone; every
        // other field falls back to its serde default.
        #[derive(Deserialize)]
        struct W {
            audio_capture: AudioCaptureSpec,
            audio_encoder: AudioEncoderSpec,
        }
        let w: W = toml::from_str(
            r#"
            [audio_capture]
            backend = "alsa"
            [audio_encoder]
            codec = "opus"
            "#,
        )
        .unwrap();
        assert_eq!(w.audio_capture.backend, "alsa");
        assert_eq!(w.audio_capture.device, "default");
        assert_eq!(w.audio_capture.sample_rate, 48_000);
        assert_eq!(w.audio_capture.channels, 2);
        assert_eq!(w.audio_encoder.codec, "opus");
        assert_eq!(w.audio_encoder.bitrate, 64_000);
        assert_eq!(w.audio_encoder.application, "audio");
        assert_eq!(w.audio_encoder.frame_ms, 20);
        assert!(w.audio_encoder.vbr);
        assert!(!w.audio_encoder.inband_fec);
        assert!(!w.audio_encoder.dtx);
        assert_eq!(w.audio_encoder.complexity, 9);
        assert!(w.audio_encoder.adaptive.is_none());
    }
}
