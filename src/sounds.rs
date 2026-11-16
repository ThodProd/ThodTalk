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
}
pub fn play(sound: Sound) {
    let wav = match sound {
        Sound::ScreenShareStart => {
            Some(&include_bytes!("../assets/sounds/confirmation_002.wav")[..])
        }
        Sound::ScreenShareStop => Some(&include_bytes!("../assets/sounds/close_002.wav")[..]),
        _ => None,
    };
    if let Some(wav) = wav {
        std::thread::spawn(move || {
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
            Sound::ScreenShareStart | Sound::ScreenShareStop => &[],
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

#[cfg(test)]
mod tests {
    #[test]
    fn share_sounds_are_wav() {
        assert!(include_bytes!("../assets/sounds/confirmation_002.wav").starts_with(b"RIFF"));
        assert!(include_bytes!("../assets/sounds/close_002.wav").starts_with(b"RIFF"));
    }
}
