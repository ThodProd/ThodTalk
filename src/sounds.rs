#[derive(Clone, Copy)]
pub enum Sound {
    Join,
    Leave,
    Sent,
    Received,
}
pub fn play(sound: Sound) {
    std::thread::spawn(move || {
        let notes: &[f32] = match sound {
            Sound::Join => &[660.0, 880.0],
            Sound::Leave => &[660.0, 440.0],
            Sound::Sent => &[880.0],
            Sound::Received => &[550.0, 740.0],
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
            PlaySoundW(
                wav.as_ptr().cast(),
                std::ptr::null_mut(),
                0x0004 | 0x0002 | 0x0010,
            );
        }
    });
}
#[cfg(windows)]
#[link(name = "winmm")]
unsafe extern "system" {
    fn PlaySoundW(sound: *const u16, module: *mut std::ffi::c_void, flags: u32) -> i32;
}
