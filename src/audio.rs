use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

pub struct Audio {
    _input: cpal::Stream,
    _output: cpal::Stream,
    pub encoded: mpsc::Receiver<Vec<u8>>,
    pub errors: mpsc::Receiver<String>,
    pub transmitting: Arc<AtomicBool>,
    pub level: Arc<std::sync::atomic::AtomicU32>,
    pub deafened: Arc<AtomicBool>,
    queues: Arc<Mutex<BTreeMap<Vec<u8>, VecDeque<f32>>>>,
    decoders: BTreeMap<Vec<u8>, opus::Decoder>,
}

pub fn devices() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
    let inputs = host
        .input_devices()
        .map(|d| d.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    let outputs = host
        .output_devices()
        .map(|d| d.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    (inputs, outputs)
}

impl Audio {
    pub fn start(input_name: &str, output_name: &str) -> Result<Self> {
        let host = cpal::default_host();
        let input = if input_name.is_empty() {
            host.default_input_device()
        } else {
            host.input_devices()?
                .find(|d| d.name().is_ok_and(|n| n == input_name))
        }
        .context("Микрофон не найден. Проверьте подключение и разрешения Windows")?;
        let output = if output_name.is_empty() {
            host.default_output_device()
        } else {
            host.output_devices()?
                .find(|d| d.name().is_ok_and(|n| n == output_name))
        }
        .context("Устройство вывода не найдено")?;
        let in_config = input.default_input_config()?;
        let out_config = output.default_output_config()?;
        let transmitting = Arc::new(AtomicBool::new(false));
        let deafened = Arc::new(AtomicBool::new(false));
        let queues: Arc<Mutex<BTreeMap<Vec<u8>, VecDeque<f32>>>> = Arc::default();
        let (frames_tx, frames_rx) = mpsc::sync_channel::<Vec<i16>>(8);
        let (encoded_tx, encoded) = mpsc::sync_channel(8);
        let (errors_tx, errors) = mpsc::sync_channel(8);
        let error_in = errors_tx.clone();
        let error_out = errors_tx.clone();
        let enabled = transmitting.clone();
        let level = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let meter = level.clone();
        let mut frame = Vec::with_capacity(960);
        let mut phase = 0u64;
        let channels = in_config.channels() as usize;
        let rate = in_config.sample_rate().0 as u64;
        let mut capture = move |samples: &[f32]| {
            if !enabled.load(Ordering::Relaxed) {
                meter.store(0, Ordering::Relaxed);
                frame.clear();
                return;
            }
            for samples in samples.chunks_exact(channels) {
                let mono = samples.iter().sum::<f32>() / channels as f32;
                phase += 48_000;
                while phase >= rate {
                    phase -= rate;
                    frame.push((mono.clamp(-1.0, 1.0) * 32767.0) as i16);
                    if frame.len() == 960 {
                        let rms = (frame
                            .iter()
                            .map(|s| (*s as f32 / 32768.0).powi(2))
                            .sum::<f32>()
                            / 960.0)
                            .sqrt();
                        meter.store(rms.to_bits(), Ordering::Relaxed);
                        let _ = frames_tx
                            .try_send(std::mem::replace(&mut frame, Vec::with_capacity(960)));
                    }
                }
            }
        };
        let input_stream = match in_config.sample_format() {
            cpal::SampleFormat::F32 => input.build_input_stream(
                &in_config.config(),
                move |data: &[f32], _| capture(data),
                move |e| {
                    let _ = error_in.try_send(format!("Микрофон: {e}"));
                },
                None,
            )?,
            cpal::SampleFormat::I16 => input.build_input_stream(
                &in_config.config(),
                move |data: &[i16], _| {
                    let samples: Vec<_> = data.iter().map(|s| *s as f32 / 32768.0).collect();
                    capture(&samples);
                },
                move |e| {
                    let _ = error_in.try_send(format!("Микрофон: {e}"));
                },
                None,
            )?,
            _ => bail!("Формат микрофона не поддерживается. Выберите другое устройство"),
        };
        let q = queues.clone();
        let silent = deafened.clone();
        let out_channels = out_config.channels() as usize;
        let out_rate = out_config.sample_rate().0 as u64;
        let mut out_phase = 0u64;
        let mut render = move |data: &mut [f32]| {
            data.fill(0.0);
            if let Ok(mut queues) = q.try_lock() {
                if silent.load(Ordering::Relaxed) {
                    for queue in queues.values_mut() {
                        queue.clear();
                    }
                    return;
                }
                for sample_frame in data.chunks_exact_mut(out_channels) {
                    let mix = queues
                        .values()
                        .filter_map(|q| q.front())
                        .sum::<f32>()
                        .clamp(-1.0, 1.0);
                    sample_frame.fill(mix);
                    out_phase += 48_000;
                    while out_phase >= out_rate {
                        out_phase -= out_rate;
                        for queue in queues.values_mut() {
                            queue.pop_front();
                        }
                    }
                }
            }
        };
        let output_stream = match out_config.sample_format() {
            cpal::SampleFormat::F32 => output.build_output_stream(
                &out_config.config(),
                move |data: &mut [f32], _| render(data),
                move |e| {
                    let _ = error_out.try_send(format!("Вывод звука: {e}"));
                },
                None,
            )?,
            cpal::SampleFormat::I16 => output.build_output_stream(
                &out_config.config(),
                move |data: &mut [i16], _| {
                    let mut samples = vec![0.0; data.len()];
                    render(&mut samples);
                    for (out, sample) in data.iter_mut().zip(samples) {
                        *out = (sample * 32767.0) as i16;
                    }
                },
                move |e| {
                    let _ = error_out.try_send(format!("Вывод звука: {e}"));
                },
                None,
            )?,
            _ => bail!("Формат вывода не поддерживается. Выберите другое устройство"),
        };
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Mono, opus::Application::Voip)?;
        encoder.set_bitrate(opus::Bitrate::Bits(24_000))?;
        encoder.set_dtx(true)?;
        std::thread::spawn(move || {
            while let Ok(frame) = frames_rx.recv() {
                let mut packet = vec![0; 4000];
                match encoder.encode(&frame, &mut packet) {
                    Ok(len) => {
                        packet.truncate(len);
                        if encoded_tx.try_send(packet).is_err() {
                            continue;
                        }
                    }
                    Err(e) => {
                        let _ = errors_tx.try_send(format!("Кодирование голоса: {e}"));
                    }
                }
            }
        });
        output_stream.play()?;
        input_stream.play()?;
        Ok(Self {
            _input: input_stream,
            _output: output_stream,
            encoded,
            errors,
            transmitting,
            level,
            deafened,
            queues,
            decoders: BTreeMap::new(),
        })
    }
    pub fn play(&mut self, author: Vec<u8>, packet: &[u8], gain: f32) -> Result<bool> {
        if self.deafened.load(Ordering::Relaxed) {
            return Ok(false);
        }
        if !self.decoders.contains_key(&author) {
            if self.decoders.len() >= 16 {
                return Ok(false);
            }
            self.decoders.insert(
                author.clone(),
                opus::Decoder::new(48_000, opus::Channels::Mono)?,
            );
        }
        let mut samples = [0i16; 5760];
        let len = self
            .decoders
            .get_mut(&author)
            .unwrap()
            .decode(packet, &mut samples, false)?;
        if let Ok(mut queues) = self.queues.lock() {
            let queue = queues.entry(author).or_default();
            if queue.is_empty() {
                queue.extend(std::iter::repeat_n(0.0, 1920));
            }
            if queue.len() > 9600 {
                queue.drain(..queue.len() - 4800);
            }
            queue.extend(
                samples[..len]
                    .iter()
                    .map(|s| *s as f32 / 32768.0 * gain.clamp(0.0, 2.0)),
            );
        }
        Ok(samples[..len].iter().any(|s| s.unsigned_abs() > 500))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "Открывает устройства на 300 мс, без захвата и передачи звука"]
    fn opens_devices_without_transmission() {
        let audio = super::Audio::start("", "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(
            !audio
                .transmitting
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        assert!(audio.encoded.try_recv().is_err());
        assert!(audio.errors.try_recv().is_err());
    }
    #[test]
    fn opus_encodes_and_decodes_voice_frame() {
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let samples: Vec<i16> = (0..960)
            .map(|n| ((n as f32 * 0.0576).sin() * 10000.0) as i16)
            .collect();
        let mut packet = [0; 4000];
        let len = encoder.encode(&samples, &mut packet).unwrap();
        let mut decoder = opus::Decoder::new(48_000, opus::Channels::Mono).unwrap();
        let mut decoded = [0; 960];
        assert_eq!(
            decoder.decode(&packet[..len], &mut decoded, false).unwrap(),
            960
        );
        assert!(decoded.iter().any(|s| *s != 0));
    }
}
