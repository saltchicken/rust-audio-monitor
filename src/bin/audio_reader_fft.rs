use clap::Parser;
use pipelink_audio_lib::{AudioReadError, AudioReader, METADATA_SIZE};
use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::sync::Arc;
use std::{mem, thread, time::Duration};

#[derive(Parser)]
#[clap(name = "audio-reader-fft")]
struct Args {
    #[clap(
        long,
        help = "Shared memory name to read from",
        default_value = "pipelink_audio_shmem"
    )]
    name: String,
}

fn main() {
    let args = Args::parse();
    // ‼️ Use the new AudioReader
    let reader = AudioReader::new(&args.name)
        .expect("Failed to open shared memory. Is the audio_monitor running?");
    println!("[AudioReaderFFT] Attached to shared memory. Waiting for data...");

    // --- Added for FFT ---
    let mut planner = FftPlanner::new();
    let mut fft_plan: Option<(usize, Arc<dyn Fft<f32>>)> = None;
    let mut complex_buffer: Vec<Complex<f32>> = Vec::new();
    // --- End FFT Setup ---

    loop {
        // ‼️ Call the new read method
        match reader.read() {
            Ok(Some(data)) => {
                // ‼️ Get the parsed data directly
                let metadata = data.metadata;
                let audio_floats = data.audio;

                // --- Print Info (Same as simple reader) ---
                let audio_data_len = audio_floats.len() * mem::size_of::<f32>();
                let total_bytes = METADATA_SIZE + audio_data_len;
                println!("[AudioReaderFFT] ✅ Read {} bytes total.", total_bytes);
                println!("  Sample Rate: {} Hz", metadata.sample_rate);
                println!("  Channels: {}", metadata.n_channels);
                println!("  Samples per Channel: {}", metadata.n_samples_per_channel);
                println!(
                    "  Audio Data Bytes: {} (Expected: {})",
                    audio_data_len, audio_data_len
                );
                println!("  Total Floats Received: {}\n", audio_floats.len());

                // --- Start FFT Calculation ---
                if metadata.n_samples_per_channel > 0 && metadata.n_channels > 0 {
                    let n_samples = metadata.n_samples_per_channel as usize;
                    let n_chans_usize = metadata.n_channels as usize;

                    // 1. Get or create the FFT plan
                    let fft = match &mut fft_plan {
                        Some((size, plan)) if *size == n_samples => plan,
                        _ => {
                            println!(
                                "[AudioReaderFFT] ‼️ Creating new FFT plan for size {}",
                                n_samples
                            );
                            let plan = planner.plan_fft_forward(n_samples);
                            fft_plan = Some((n_samples, plan));
                            &fft_plan.as_mut().unwrap().1
                        }
                    };

                    // 2. Prepare the complex buffer.
                    complex_buffer.clear();
                    complex_buffer.resize(n_samples, Complex::default());

                    // ‼️ This is now much simpler! We already have &[f32]
                    // No need for bytemuck::cast_slice here.
                    for (i, sample_f32) in audio_floats
                        .iter()
                        .step_by(n_chans_usize) // Take every Nth sample (e.g., [L], R, [L], R)
                        .enumerate()
                        .take(n_samples)
                    // Ensure we don't go past the buffer size
                    {
                        complex_buffer[i] = Complex {
                            re: *sample_f32,
                            im: 0.0,
                        };
                    }

                    // 3. Run the FFT
                    fft.process(&mut complex_buffer);

                    // 4. Find the peak frequency
                    let (peak_bin_index, peak_magnitude) = complex_buffer[..n_samples / 2]
                        .iter()
                        .enumerate()
                        .map(|(i, c)| (i, c.norm()))
                        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                        .unwrap_or((0, 0.0));

                    // 5. Convert the bin index back to a frequency
                    let bin_width = metadata.sample_rate / (n_samples as f32);
                    let peak_frequency = peak_bin_index as f32 * bin_width;

                    println!(
                        "  [FFT] ‼️ Peak Frequency (Ch 0): {:.2} Hz (Magnitude: {:.2})\n",
                        peak_frequency, peak_magnitude
                    );
                }
                // --- End FFT Calculation ---
            }
            Ok(None) => {
                // No new data, just wait.
            }
            Err(e) => {
                // ‼️ Use the same robust error handling
                match e {
                    AudioReadError::Shmem(shmem_err) => {
                        eprintln!("[AudioReaderFFT] ❌ Shared memory error: {}", shmem_err);
                        break;
                    }
                    AudioReadError::DataTooSmall { needed, got } => {
                        eprintln!(
                            "[AudioReaderFFT] ⚠️ Parse error: Data too small. Needed {}, got {}",
                            needed, got
                        );
                    }
                    AudioReadError::DataMismatch { expected, got } => {
                        eprintln!(
                            "[AudioReaderFFT] ⚠️ Parse error: Data mismatch. Expected {} audio bytes, got {}",
                            expected, got
                        );
                    }
                    AudioReadError::InvalidMetadata(_) | AudioReadError::InvalidAudioData(_) => {
                        eprintln!(
                            "[AudioReaderFFT] ⚠️ Parse error: Data is corrupted and cannot be cast."
                        );
                    }
                }
            }
        }
        // Poll for new data
        thread::sleep(Duration::from_millis(1));
    }
}
