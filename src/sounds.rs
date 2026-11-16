#[derive(Clone, Copy)]
pub enum Sound {
    Join,
    Leave,
    VoiceJoin,
    VoiceLeave,
    Mute,
    Unmute,
    Deafen,
    Undeafen,
    Sent,
    Received,
    MessageMention,
    UserJoinChannel,
    UserLeaveChannel,
    Error,
    ScreenShareStart,
    ScreenShareStop,
    ScreenViewerJoin,
    ScreenViewerLeave,
}
pub fn play(sound: Sound) {
    let wav = match sound {
        Sound::ScreenShareStart => Some((
            &include_bytes!("../assets/sounds/confirmation_002.wav")[..],
            0.42,
            2400.0,
        )),
        Sound::ScreenShareStop => Some((
            &include_bytes!("../assets/sounds/close_002.wav")[..],
            0.24,
            1300.0,
        )),
        Sound::ScreenViewerJoin => Some((
            &include_bytes!("../assets/sounds/confirmation_002.wav")[..],
            0.30,
            2100.0,
        )),
        Sound::ScreenViewerLeave => Some((
            &include_bytes!("../assets/sounds/close_002.wav")[..],
            0.18,
            1100.0,
        )),
        _ => None,
    };
    if let Some((wav, gain, cutoff)) = wav {
        std::thread::spawn(move || {
            // Lower the level and soften the highs so the cue is gentle rather
            // than a sharp click. The WAV header is kept intact.
            let wav = soften_wav(wav, gain, cutoff);
            #[cfg(windows)]
            unsafe {
                PlaySoundW(wav.as_ptr().cast(), std::ptr::null_mut(), 0x0004 | 0x0002);
            }
            #[cfg(not(windows))]
            let _ = wav;
        });
        return;
    }
    std::thread::spawn(move || {
        let notes: &[f32] = match sound {
            Sound::Join => &[660.0, 880.0],
            Sound::Leave => &[660.0, 440.0],
            Sound::VoiceJoin => &[740.0, 988.0],
            Sound::VoiceLeave => &[740.0, 554.0],
            Sound::Mute => &[520.0, 390.0],
            Sound::Unmute => &[390.0, 520.0],
            Sound::Deafen => &[440.0, 330.0],
            Sound::Undeafen => &[330.0, 440.0],
            Sound::Sent => &[880.0, 880.0],
            Sound::Received => &[550.0, 740.0],
            Sound::MessageMention => &[800.0, 1000.0, 800.0],
            Sound::UserJoinChannel => &[700.0, 900.0],
            Sound::UserLeaveChannel => &[700.0, 500.0],
            Sound::Error => &[400.0, 300.0],
            Sound::ScreenShareStart
            | Sound::ScreenShareStop
            | Sound::ScreenViewerJoin
            | Sound::ScreenViewerLeave => &[],
        };
        let rate = 22050u32;
        let n = rate as usize * 65 / 1000;
        let mut pcm = Vec::new();
        for &hz in notes {
            for i in 0..n {
                let envelope = (std::f32::consts::PI * i as f32 / n as f32).sin().powi(2);
                let s = ((i as f32 * hz * std::f32::consts::TAU / rate as f32).sin()
                    * envelope
                    * 1800.0) as i16;
                pcm.extend(s.to_le_bytes());
            }
        }
        let mut wav = b"RIFF".to_vec();
        wav.extend((36 + pcm.len() as u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(rate.to_le_bytes());
        wav.extend((rate * 2).to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((pcm.len() as u32).to_le_bytes());
        wav.extend(pcm);
        #[cfg(windows)]
        unsafe {
            PlaySoundW(wav.as_ptr().cast(), std::ptr::null_mut(), 0x0004 | 0x0002);
        }
    });
}
#[cfg(windows)]
#[link(name = "winmm")]
unsafe extern "system" {
    fn PlaySoundW(sound: *const u16, module: *mut std::ffi::c_void, flags: u32) -> i32;
}

/// Find the `data` chunk of a PCM WAV file.
fn wav_data_range(wav: &[u8]) -> Option<(usize, usize)> {
    if wav.len() < 12 || &wav[..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    let mut format_ok = false;
    while pos + 8 <= wav.len() {
        let id = &wav[pos..pos + 4];
        let len =
            u32::from_le_bytes([wav[pos + 4], wav[pos + 5], wav[pos + 6], wav[pos + 7]]) as usize;
        let start = pos + 8;
        let end = start.saturating_add(len).min(wav.len());
        if id == b"fmt " && start + 16 <= wav.len() {
            let format = u16::from_le_bytes([wav[start], wav[start + 1]]);
            let bits = u16::from_le_bytes([wav[start + 14], wav[start + 15]]);
            format_ok = format == 1 && bits == 16;
        } else if id == b"data" {
            return format_ok.then_some((start, end));
        }
        pos = end.saturating_add(len & 1);
    }
    None
}

/// Apply gain, a one-pole low-pass filter and short fades to a PCM WAV.
fn soften_wav(wav: &[u8], gain: f32, cutoff_hz: f32) -> Vec<u8> {
    let mut out = wav.to_vec();
    let Some((start, end)) = wav_data_range(&out) else {
        return out;
    };
    if out.len() < 36 || end <= start {
        return out;
    }
    let channels = u16::from_le_bytes([out[22], out[23]]).max(1) as usize;
    let rate = u32::from_le_bytes([out[24], out[25], out[26], out[27]]) as f32;
    if rate <= 0.0 {
        return out;
    }
    let frames = (end - start) / 2 / channels;
    if frames == 0 {
        return out;
    }
    let alpha = 1.0 - (-std::f32::consts::TAU * cutoff_hz / rate).exp();
    let fade_in = ((rate * 0.012) as usize).clamp(1, frames);
    let fade_out = ((rate * 0.070) as usize).clamp(1, frames);
    let mut low = vec![0.0f32; channels];
    for frame in 0..frames {
        let envelope = (frame as f32 / fade_in as f32).min(1.0)
            * ((frames - 1 - frame) as f32 / fade_out as f32).min(1.0);
        for (channel, state) in low.iter_mut().enumerate() {
            let index = start + (frame * channels + channel) * 2;
            if index + 1 >= end {
                break;
            }
            let input = i16::from_le_bytes([out[index], out[index + 1]]) as f32 / 32768.0;
            *state += alpha * (input - *state);
            let sample = (*state * gain * envelope * 32767.0)
                .round()
                .clamp(-32768.0, 32767.0) as i16;
            out[index..index + 2].copy_from_slice(&sample.to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{soften_wav, wav_data_range};

    fn peak(wav: &[u8]) -> u16 {
        let (start, end) = wav_data_range(wav).expect("data chunk");
        (start..end.saturating_sub(1))
            .step_by(2)
            .map(|i| i16::from_le_bytes([wav[i], wav[i + 1]]).unsigned_abs())
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn share_sounds_are_wav() {
        assert!(include_bytes!("../assets/sounds/confirmation_002.wav").starts_with(b"RIFF"));
        assert!(include_bytes!("../assets/sounds/close_002.wav").starts_with(b"RIFF"));
    }

    #[test]
    fn share_sounds_are_softened() {
        let raw = include_bytes!("../assets/sounds/confirmation_002.wav");
        let soft = soften_wav(raw, 0.42, 2400.0);
        assert_eq!(&soft[..4], b"RIFF");
        assert_eq!(soft.len(), raw.len());
        assert!(peak(&soft) < peak(raw));
        let raw = include_bytes!("../assets/sounds/close_002.wav");
        let soft = soften_wav(raw, 0.24, 1300.0);
        assert_eq!(&soft[..4], b"RIFF");
        assert!(peak(&soft) < peak(raw));
    }
}
