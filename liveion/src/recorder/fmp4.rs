// fmp4.rs – minimal fragmented-MP4 (fMP4) writer
// This intentionally supports only the subset needed by the recorder right now
// (one H.264/AVC video track) but is designed to be codec-agnostic so it can be
// extended later. The focus is to remove the dependency on the external `mp4`
// crate while still emitting a standards-compliant init segment (ftyp+moov [+mvex]).

use byteorder::{BigEndian, ByteOrder};
use bytes::Bytes;

/// A raw media sample description understood by the recorder.
/// This mirrors the public fields that were previously taken from `mp4::Mp4Sample`.
#[derive(Clone, Debug)]
pub struct Mp4Sample {
    pub start_time: u64,
    pub duration: u32,
    pub rendering_offset: i32,
    pub is_sync: bool,
    pub bytes: Bytes,
}

/// Writer for a single-track fragmented-MP4 stream.
/// Currently only supports a *video* track (type `vide`) but does not hard-code
/// any codec assumptions beyond carrying an opaque configuration box.
pub struct Fmp4Writer {
    pub timescale: u32,
    pub track_id: u32,
    pub width: u32,
    pub height: u32,
    pub channels: u16,
    pub sample_rate: u32,
    pub kind: TrackKind,
    pub codec_string: String, // e.g. "avc1.42E01E" or "hev1.1.6.L93.90" or "opus"
    // Raw codec private blobs that should be put into the sample entry's
    // codec-specific configuration box (e.g. SPS/PPS for AVC).
    pub codec_config: Vec<Vec<u8>>, // Usually [sps, pps] for AVC
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrackKind {
    Video,
    Audio,
}

impl Fmp4Writer {
    pub fn new(
        timescale: u32,
        track_id: u32,
        width: u32,
        height: u32,
        codec_string: String,
        codec_config: Vec<Vec<u8>>, // ordered blobs
    ) -> Self {
        Self {
            timescale,
            track_id,
            width,
            height,
            channels: 0,
            sample_rate: 0,
            kind: TrackKind::Video,
            codec_string,
            codec_config,
        }
    }

    /// Create an *audio* track (Opus).
    pub fn new_audio(
        timescale: u32,
        track_id: u32,
        channels: u16,
        sample_rate: u32,
        codec_string: String,
        codec_config: Vec<Vec<u8>>, // usually contains OpusHead in dOps
    ) -> Self {
        Self {
            timescale,
            track_id,
            width: 0,
            height: 0,
            channels,
            sample_rate,
            kind: TrackKind::Audio,
            codec_string,
            codec_config,
        }
    }

    /// Build a standalone *initialisation segment* (`init.m4s`) consisting of
    /// `ftyp` + `moov` (+ `mvex/trex`).
    pub fn build_init_segment(&self) -> Vec<u8> {
        let ftyp = self.build_ftyp();
        let moov = self.build_moov();

        let mut out = Vec::with_capacity(ftyp.len() + moov.len());
        out.extend_from_slice(&ftyp);
        out.extend_from_slice(&moov);
        out
    }

    // === internal helpers ===

    fn build_ftyp(&self) -> Vec<u8> {
        // major_brand = isom, minor_version = 512, compat = isom iso2 avc1 mp41
        let mut payload = Vec::with_capacity(4 + 4 + 4 * 4);
        payload.extend_from_slice(b"isom");
        payload.extend_from_slice(&512u32.to_be_bytes());
        payload.extend_from_slice(b"isom");
        payload.extend_from_slice(b"iso2");
        payload.extend_from_slice(b"avc1");
        payload.extend_from_slice(b"mp41");
        make_box(b"ftyp", &payload)
    }

    fn build_moov(&self) -> Vec<u8> {
        let mvhd = build_mvhd(self.timescale, self.track_id + 1); // nextTrackID
        let trak = self.build_trak();
        let mvex = build_mvex(self.track_id);

        let mut payload = Vec::with_capacity(mvhd.len() + trak.len() + mvex.len());
        payload.extend_from_slice(&mvhd);
        payload.extend_from_slice(&trak);
        payload.extend_from_slice(&mvex);

        make_box(b"moov", &payload)
    }

    fn build_trak(&self) -> Vec<u8> {
        let tkhd = build_tkhd(self.track_id, self.width, self.height);
        let mdia = self.build_mdia();

        let mut payload = Vec::with_capacity(tkhd.len() + mdia.len());
        payload.extend_from_slice(&tkhd);
        payload.extend_from_slice(&mdia);
        make_box(b"trak", &payload)
    }

    fn build_mdia(&self) -> Vec<u8> {
        let mdhd = build_mdhd(self.timescale);
        let hdlr = if self.kind == TrackKind::Video {
            build_hdlr(b"vide", b"VideoHandler\0")
        } else {
            build_hdlr(b"soun", b"SoundHandler\0")
        };
        let minf = self.build_minf();

        let mut payload = Vec::with_capacity(mdhd.len() + hdlr.len() + minf.len());
        payload.extend_from_slice(&mdhd);
        payload.extend_from_slice(&hdlr);
        payload.extend_from_slice(&minf);
        make_box(b"mdia", &payload)
    }

    fn build_minf(&self) -> Vec<u8> {
        let header = if self.kind == TrackKind::Video {
            build_vmhd()
        } else {
            build_smhd()
        };
        let dinf = build_dinf();
        let stbl = self.build_stbl();

        let mut payload = Vec::with_capacity(header.len() + dinf.len() + stbl.len());
        payload.extend_from_slice(&header);
        payload.extend_from_slice(&dinf);
        payload.extend_from_slice(&stbl);
        make_box(b"minf", &payload)
    }

    fn build_stbl(&self) -> Vec<u8> {
        let stsd = self.build_stsd();
        let stts = build_empty_full_box(b"stts");
        let stsc = build_empty_full_box(b"stsc");
        let stsz = build_empty_stsz();
        let stco = build_empty_full_box(b"stco");

        let mut payload = Vec::with_capacity(
            stsd.len() + stts.len() + stsc.len() + stsz.len() + stco.len(),
        );
        payload.extend_from_slice(&stsd);
        payload.extend_from_slice(&stts);
        payload.extend_from_slice(&stsc);
        payload.extend_from_slice(&stsz);
        payload.extend_from_slice(&stco);
        make_box(b"stbl", &payload)
    }

    fn build_stsd(&self) -> Vec<u8> {
        // Only one entry – sample entry depending on track kind
        let sample_entry = if self.kind == TrackKind::Video {
            self.build_avc1_sample_entry()
        } else {
            self.build_opus_sample_entry()
        };

        let mut payload = Vec::with_capacity(4 + 4 + sample_entry.len());
        payload.extend_from_slice(&0u32.to_be_bytes()); // version & flags
        payload.extend_from_slice(&1u32.to_be_bytes()); // entry_count
        payload.extend_from_slice(&sample_entry);
        make_box(b"stsd", &payload)
    }

    fn build_avc1_sample_entry(&self) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0u8; 6]); // reserved
        payload.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index

        // pre_defined & reserved (16+32*3 bits)
        payload.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
        payload.extend_from_slice(&0u16.to_be_bytes()); // reserved
        payload.extend_from_slice(&0u32.to_be_bytes()); // pre_defined[0]
        payload.extend_from_slice(&0u32.to_be_bytes()); // pre_defined[1]
        payload.extend_from_slice(&0u32.to_be_bytes()); // pre_defined[2]

        // width/height
        payload.extend_from_slice(&(self.width as u16).to_be_bytes());
        payload.extend_from_slice(&(self.height as u16).to_be_bytes());

        // horiz & vert resolution (72 dpi)
        payload.extend_from_slice(&0x0048_0000u32.to_be_bytes());
        payload.extend_from_slice(&0x0048_0000u32.to_be_bytes());

        payload.extend_from_slice(&0u32.to_be_bytes()); // reserved
        payload.extend_from_slice(&1u16.to_be_bytes()); // frame_count

        // compressor name (32 bytes)
        payload.extend_from_slice(&[0u8; 32]);

        payload.extend_from_slice(&0x0018u16.to_be_bytes()); // depth
        payload.extend_from_slice(&0xFFFFu16.to_be_bytes()); // pre_defined

        // avcC box containing codec config (SPS/PPS)
        let avcc = build_avcc(&self.codec_config);
        payload.extend_from_slice(&avcc);

        make_box(b"avc1", &payload)
    }

    fn build_opus_sample_entry(&self) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0u8; 6]); // reserved
        payload.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index

        // reserved
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());

        // channelcount & samplesize
        payload.extend_from_slice(&self.channels.to_be_bytes());
        payload.extend_from_slice(&16u16.to_be_bytes()); // sample size 16-bit placeholder

        // pre_defined & reserved
        payload.extend_from_slice(&0u16.to_be_bytes());
        payload.extend_from_slice(&0u16.to_be_bytes());

        // samplerate 32-bit fixed (sampleRate<<16)
        let fixed_rate = (self.sample_rate as u32) << 16;
        payload.extend_from_slice(&fixed_rate.to_be_bytes());

        // Minimal dOps box
        let mut dops_payload = Vec::new();
        dops_payload.push(0u8); // version
        dops_payload.push(self.channels as u8); // output channel count
        dops_payload.extend_from_slice(&0u16.to_be_bytes()); // pre-skip
        dops_payload.extend_from_slice(&self.sample_rate.to_be_bytes()); // input sample rate
        dops_payload.extend_from_slice(&0i16.to_be_bytes()); // output gain
        dops_payload.push(0u8); // channel mapping family
        let dops = make_box(b"dOps", &dops_payload);
        payload.extend_from_slice(&dops);

        make_box(b"Opus", &payload)
    }

    /// Build a media fragment (styp+moof+mdat) for the given samples using this writer's track id.
    pub fn build_fragment(
        &self,
        seq_number: u32,
        base_time: u64,
        samples: &[Mp4Sample],
    ) -> Vec<u8> {
        _build_fragment_internal(self.track_id, seq_number, base_time, samples)
    }
}

// ======================= standalone box builders ===========================

fn build_mvhd(timescale: u32, next_track_id: u32) -> Vec<u8> {
    let mut payload = Vec::with_capacity(100);
    be_u32(&mut payload, 0);                // version & flags
    zeroes(&mut payload, 8);                // creation & modification time
    be_u32(&mut payload, timescale);
    be_u32(&mut payload, 0);                // duration unknown
    be_u32(&mut payload, 0x0001_0000);      // rate 1.0
    be_u16(&mut payload, 0x0100);           // volume 1.0
    be_u16(&mut payload, 0);                // reserved
    zeroes(&mut payload, 8);                // reserved

    // unity matrix (identity)
    be_u32(&mut payload, 0x0001_0000);
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0x0001_0000);
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0x4000_0000);

    zeroes(&mut payload, 24);               // pre_defined[6]
    be_u32(&mut payload, next_track_id);
    make_box(b"mvhd", &payload)
}

fn build_tkhd(track_id: u32, width: u32, height: u32) -> Vec<u8> {
    let mut payload = Vec::with_capacity(92);
    be_u32(&mut payload, 0x0000_0007);      // version & flags
    zeroes(&mut payload, 8);                // creation & modification time
    be_u32(&mut payload, track_id);
    be_u32(&mut payload, 0);                // reserved
    be_u32(&mut payload, 0);                // duration
    zeroes(&mut payload, 8);                // reserved
    be_u16(&mut payload, 0);                // layer
    be_u16(&mut payload, 0);                // alternate group
    be_u16(&mut payload, 0);                // volume (mute)
    be_u16(&mut payload, 0);                // reserved

    // unity matrix
    be_u32(&mut payload, 0x0001_0000);
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0x0001_0000);
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0x4000_0000);

    // width/height 16.16 fixed
    be_u32(&mut payload, (width << 16) as u32);
    be_u32(&mut payload, (height << 16) as u32);

    make_box(b"tkhd", &payload)
}

fn build_mdhd(timescale: u32) -> Vec<u8> {
    let mut payload = Vec::with_capacity(32);
    be_u32(&mut payload, 0);                // version & flags
    zeroes(&mut payload, 8);                // creation & modification time
    be_u32(&mut payload, timescale);
    be_u32(&mut payload, 0);                // duration
    be_u16(&mut payload, 0);                // language (und)
    be_u16(&mut payload, 0);                // pre_defined
    make_box(b"mdhd", &payload)
}

fn build_hdlr(typ: &[u8; 4], name: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(32 + name.len());
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0);
    payload.extend_from_slice(typ);
    zeroes(&mut payload, 12);
    payload.extend_from_slice(name);
    make_box(b"hdlr", &payload)
}

fn build_vmhd() -> Vec<u8> {
    let mut payload = Vec::with_capacity(12);
    be_u32(&mut payload, 0x0000_0001);      // version & flags
    be_u16(&mut payload, 0);                // graphics_mode
    be_u16(&mut payload, 0);
    be_u16(&mut payload, 0);
    be_u16(&mut payload, 0);
    make_box(b"vmhd", &payload)
}

fn build_smhd() -> Vec<u8> {
    let mut payload = Vec::with_capacity(12);
    be_u32(&mut payload, 0x0000_0001);      // version & flags
    be_u16(&mut payload, 0);                // balance
    be_u16(&mut payload, 0);
    be_u16(&mut payload, 0);
    be_u16(&mut payload, 0);
    make_box(b"smhd", &payload)
}

fn build_dinf() -> Vec<u8> {
    let dref = {
        let url_box = {
            let mut payload = Vec::with_capacity(4);
            be_u32(&mut payload, 0x0000_0001);        // version 0 + flags 1 (self-contained)
            make_box(b"url ", &payload)
        };

        let mut payload = Vec::with_capacity(8 + url_box.len());
        be_u32(&mut payload, 0);                     // version & flags
        be_u32(&mut payload, 1);                     // entry_count
        payload.extend_from_slice(&url_box);
        make_box(b"dref", &payload)
    };

    make_box(b"dinf", &dref)
}

fn build_empty_full_box(typ: &[u8; 4]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(8);
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0);
    make_box(typ, &payload)
}

fn build_empty_stsz() -> Vec<u8> {
    let mut payload = Vec::with_capacity(12);
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0);
    be_u32(&mut payload, 0);
    make_box(b"stsz", &payload)
}

// --- avcC builder (SPS/PPS) ---
/// `config_blobs` should contain at least an SPS followed by one or more PPS blobs.
fn build_avcc(config_blobs: &[Vec<u8>]) -> Vec<u8> {
    if config_blobs.is_empty() {
        return make_box(b"avcC", &[]);
    }

    let sps = &config_blobs[0];
    let pps_list = &config_blobs[1..];

    let mut payload = Vec::new();
    // configurationVersion, AVCProfileIndication, profile_compatibility, AVCLevelIndication
    payload.push(1u8);
    payload.push(*sps.get(1).unwrap_or(&0)); // profile
    payload.push(*sps.get(2).unwrap_or(&0)); // compatibility
    payload.push(*sps.get(3).unwrap_or(&0)); // level

    // 6 bits reserved (all on) + 2 bits lengthSizeMinusOne (3 for 4-byte lengths)
    payload.push(0xFFu8);

    // 3 bits reserved + 5 bits numOfSequenceParameterSets (usually 1)
    payload.push(0xE0u8 | 1);

    // SPS
    payload.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    payload.extend_from_slice(sps);

    // PPS count
    payload.push(pps_list.len() as u8);
    for pps in pps_list {
        payload.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        payload.extend_from_slice(pps);
    }

    make_box(b"avcC", &payload)
}

// --- mvex / trex (defaults for fragmented MP4) ---
fn build_mvex(track_id: u32) -> Vec<u8> {
    let trex_size: u32 = 32;
    let mvex_size: u32 = 8 + trex_size;

    let mut buf = Vec::with_capacity(mvex_size as usize);
    be_u32(&mut buf, mvex_size);
    buf.extend_from_slice(b"mvex");

    be_u32(&mut buf, trex_size);
    buf.extend_from_slice(b"trex");
    be_u32(&mut buf, 0);                 // version & flags
    be_u32(&mut buf, track_id);
    be_u32(&mut buf, 1);                 // default_sample_description_index
    be_u32(&mut buf, 0);                 // default_sample_duration
    be_u32(&mut buf, 0);                 // default_sample_size
    be_u32(&mut buf, 0x0101_0000);       // default flags
    buf
}

// ======================= generic helpers ===================================
fn make_box(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + payload.len());
    let size = (8 + payload.len()) as u32;
    v.extend_from_slice(&size.to_be_bytes());
    v.extend_from_slice(typ);
    v.extend_from_slice(payload);
    v
}

/// Build a `styp` + `moof` + `mdat` fragment for the provided samples.
///
/// * `track_id`     – ID of the track the samples belong to (usually 1)
/// * `seq_number`   – monotonically increasing sequence number (starts at 1)
/// * `base_time`    – decode timestamp (DTS) of the first sample in this fragment
/// * `samples`      – list of media samples already converted to length-prefixed
///                     AVCC format (for AVC) or other 4-byte-length-prefixed
///                     RAW format the decoder expects.
fn _build_fragment_internal(
    track_id: u32,
    seq_number: u32,
    base_time: u64,
    samples: &[Mp4Sample],
) -> Vec<u8> {
    let total_data: usize = samples.iter().map(|s| s.bytes.len()).sum();

    // ========= styp =========
    let mut fragment: Vec<u8> = Vec::with_capacity(1024 + total_data);
    const STYP_SIZE: u32 = 24;
    fragment.extend_from_slice(&STYP_SIZE.to_be_bytes());
    fragment.extend_from_slice(b"styp");
    fragment.extend_from_slice(b"msdh");
    fragment.extend_from_slice(&0u32.to_be_bytes()); // minor version
    fragment.extend_from_slice(b"msdh");
    fragment.extend_from_slice(b"dash");

    // ========= moof =========
    let moof_start = fragment.len();
    fragment.extend_from_slice(&[0u8; 8]); // placeholder for moof size+type

    // ---- mfhd ----
    fragment.extend_from_slice(&16u32.to_be_bytes());
    fragment.extend_from_slice(b"mfhd");
    fragment.extend_from_slice(&0u32.to_be_bytes());
    fragment.extend_from_slice(&seq_number.to_be_bytes());

    // ---- traf ----
    let traf_start = fragment.len();
    fragment.extend_from_slice(&[0u8; 8]); // placeholder traf header

    // tfhd
    let tfhd_flags: u32 = 0x000200; // default-base-is-moof
    fragment.extend_from_slice(&16u32.to_be_bytes());
    fragment.extend_from_slice(b"tfhd");
    fragment.extend_from_slice(&tfhd_flags.to_be_bytes());
    fragment.extend_from_slice(&track_id.to_be_bytes());

    // tfdt (version 1)
    fragment.extend_from_slice(&20u32.to_be_bytes());
    fragment.extend_from_slice(b"tfdt");
    fragment.extend_from_slice(&0x01000000u32.to_be_bytes());
    fragment.extend_from_slice(&base_time.to_be_bytes());

    // trun
    let sample_count = samples.len() as u32;
    // trun flags: data-offset present | sample-duration present | sample-size present | sample-flags present
    let trun_flags: u32 = 0x000001 | 0x000100 | 0x000200 | 0x000400;
    let trun_start = fragment.len();
    fragment.extend_from_slice(&[0u8; 4]); // placeholder size
    fragment.extend_from_slice(b"trun");
    fragment.extend_from_slice(&trun_flags.to_be_bytes());
    fragment.extend_from_slice(&sample_count.to_be_bytes());
    let data_offset_pos = fragment.len();
    fragment.extend_from_slice(&[0u8; 4]); // data offset placeholder

    for s in samples {
        fragment.extend_from_slice(&s.duration.to_be_bytes());
        fragment.extend_from_slice(&(s.bytes.len() as u32).to_be_bytes());
        // Sample flags: mark sync vs non-sync samples
        let flags: u32 = if s.is_sync {
            // sample_depends_on = 2 (no dependencies, key frame)
            0x0200_0000
        } else {
            // sample_depends_on = 1, sample_is_non_sync = 1
            0x0101_0000
        };
        fragment.extend_from_slice(&flags.to_be_bytes());
    }

    // patch sizes
    let trun_size = (fragment.len() - trun_start) as u32;
    BigEndian::write_u32(&mut fragment[trun_start..trun_start + 4], trun_size);

    let traf_size = (fragment.len() - traf_start) as u32;
    BigEndian::write_u32(&mut fragment[traf_start..traf_start + 4], traf_size);
    fragment[traf_start + 4..traf_start + 8].copy_from_slice(b"traf");

    let moof_size = (fragment.len() - moof_start) as u32;
    BigEndian::write_u32(&mut fragment[moof_start..moof_start + 4], moof_size);
    fragment[moof_start + 4..moof_start + 8].copy_from_slice(b"moof");

    // data-offset: distance from moof start to first byte of mdat payload
    let data_offset_val = moof_size + 8; // mdat header
    BigEndian::write_u32(
        &mut fragment[data_offset_pos..data_offset_pos + 4],
        data_offset_val,
    );

    // ========= mdat =========
    let mdat_size = (8 + total_data) as u32;
    fragment.extend_from_slice(&mdat_size.to_be_bytes());
    fragment.extend_from_slice(b"mdat");
    for s in samples {
        fragment.extend_from_slice(s.bytes.as_ref());
    }

    fragment
}

// Helpers for big-endian writing & padding -------------------------------
#[inline]
fn be_u8(buf: &mut Vec<u8>, v: u8) { buf.push(v); }
#[inline]
fn be_u16(buf: &mut Vec<u8>, v: u16) { buf.extend_from_slice(&v.to_be_bytes()); }
#[inline]
fn be_u32(buf: &mut Vec<u8>, v: u32) { buf.extend_from_slice(&v.to_be_bytes()); }
#[inline]
fn be_u64(buf: &mut Vec<u8>, v: u64) { buf.extend_from_slice(&v.to_be_bytes()); }
#[inline]
fn zeroes(buf: &mut Vec<u8>, n: usize) { buf.extend(std::iter::repeat(0u8).take(n)); }

/// Convert an Annex-B NALU (with or without start code) to a 4-byte-length-prefixed AVCC buffer.
pub fn nalu_to_avcc(nalu: &Bytes) -> Vec<u8> {
    // Determine where the raw payload starts (skip 3- or 4-byte start code if present)
    let offset = if nalu.len() >= 4 && nalu[..4] == [0, 0, 0, 1][..] {
        4
    } else if nalu.len() >= 3 && nalu[..3] == [0, 0, 1][..] {
        3
    } else {
        0
    };
    let payload = &nalu[offset..];
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
} 