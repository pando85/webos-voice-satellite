//! In-process decoding (mp3, wav/pcm, flac, ogg/vorbis) with symphonia.

use std::io::Cursor;

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

use crate::error::PlaybackError;
use crate::pcm::Pcm;

/// Cap on decoded samples (about five minutes of 48 kHz stereo) so a small
/// compressed body cannot expand into an unbounded buffer.
pub const MAX_DECODED_SAMPLES: usize = 48_000 * 2 * 300;

/// Decodes a whole in-memory file into interleaved 16-bit PCM.
///
/// `hint` is an optional file extension (`"mp3"`, `"wav"`, `"flac"`) or MIME
/// type (`"audio/mpeg"`). It only speeds up probing; the format is detected
/// from the content either way.
///
/// # Errors
/// [`PlaybackError::Decode`] when the bytes are not decodable audio and
/// [`PlaybackError::Unsupported`] for empty or format-changing streams.
pub fn decode(bytes: &[u8], hint: Option<&str>) -> Result<Pcm, PlaybackError> {
    let mut h = Hint::new();
    if let Some(s) = hint {
        let s = s.trim().to_ascii_lowercase();
        if s.contains('/') {
            h.mime_type(&s);
        } else if !s.is_empty() {
            h.with_extension(s.trim_start_matches('.'));
        }
    }
    let mss = MediaSourceStream::new(
        Box::new(Cursor::new(bytes.to_vec())),
        MediaSourceStreamOptions::default(),
    );
    let mut format = symphonia::default::get_probe()
        .probe(
            &h,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| PlaybackError::Decode(format!("unrecognised audio: {e}")))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| PlaybackError::Decode("no audio track".into()))?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| PlaybackError::Decode("no audio codec parameters".into()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| PlaybackError::Decode(format!("no decoder: {e}")))?;
    let track_id = track.id;

    let mut spec: Option<(u32, u16)> = None;
    let mut samples: Vec<i16> = Vec::new();
    let mut chunk: Vec<i16> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(PlaybackError::Decode(e.to_string())),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                let this = (
                    buf.spec().rate(),
                    u16::try_from(buf.spec().channels().count()).unwrap_or(0),
                );
                match spec {
                    None => spec = Some(this),
                    Some(s) if s != this => {
                        return Err(PlaybackError::Unsupported(
                            "sample rate or channel count changes mid-stream".into(),
                        ));
                    }
                    Some(_) => {}
                }
                buf.copy_to_vec_interleaved(&mut chunk);
                samples.extend_from_slice(&chunk);
                if samples.len() > MAX_DECODED_SAMPLES {
                    return Err(PlaybackError::Unsupported("decoded audio too long".into()));
                }
            }
            // A corrupt packet is skipped; the rest of the stream may be fine.
            Err(SymError::DecodeError(_)) => {}
            Err(e) => return Err(PlaybackError::Decode(e.to_string())),
        }
    }
    match spec {
        Some((rate, channels)) if rate > 0 && channels > 0 && !samples.is_empty() => Ok(Pcm {
            rate,
            channels,
            samples,
        }),
        _ => Err(PlaybackError::Decode("no audio frames decoded".into())),
    }
}
