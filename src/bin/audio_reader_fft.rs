use clap::Parser;
use pipelink_audio_lib::{AudioMetadata, METADATA_SIZE};
use proclink::ShmemReader;
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
    let reader = ShmemReader::new(&args.name)
        .expect("Failed to open shared memory. Is the audio_monitor running?");
    println!("[AudioReaderFFT] Attached to shared memory. Waiting for data...");

    // --- Added for FFT ---
    let mut planner = FftPlanner::new();
    let mut fft_plan: Option<(usize, Arc<dyn Fft<f32>>)> = None;
    let mut complex_buffer: Vec<Complex<f32>> = Vec::new();
    // --- End FFT Setup ---

    loop {
        match reader.read() {
            Ok(Some(data)) => {
                // 1. Check if we have enough data for the metadata header
                if data.len() < METADATA_SIZE {
                    println!(
                        "[AudioReaderFFT] ⚠️ Received data is too small for metadata! Need {}, got {}",
                        METADATA_SIZE,
                        data.len()
                    );
                    continue;
                }

                // 2. Split the data into metadata and audio
                let (metadata_bytes, audio_data) = data.split_at(METADATA_SIZE);

                // 3. Cast the metadata bytes into our struct
                let metadata: &AudioMetadata = bytemuck::from_bytes(metadata_bytes);

                // 4. Get audio data info
                let audio_data_len = audio_data.len();

                // 5. Calculate expected vs. received
                let expected_bytes = (metadata.n_samples_per_channel
                    * metadata.n_channels
                    * mem::size_of::<f32>() as u32) as usize;

                let num_floats_received = audio_data_len / mem::size_of::<f32>();

                // Print the info (as before)
                println!("[AudioReaderFFT] ✅ Read {} bytes total.", data.len());
                println!("  Sample Rate: {} Hz", metadata.sample_rate);
                println!("  Channels: {}", metadata.n_channels);
                println!("  Samples per Channel: {}", metadata.n_samples_per_channel);
                println!(
                    "  Audio Data Bytes: {} (Expected: {})",
                    audio_data_len, expected_bytes
                );
                println!("  Total Floats Received: {}\n", num_floats_received);

                // Simple validation (as before)
                if audio_data_len != expected_bytes {
                    println!(
                        "[AudioReaderFFT] ⚠️ WARNING: Received audio data size does not match metadata!"
                    );
                }

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

                    // Cast the raw audio data bytes to a slice of f32.
                    // This is safe because we know the sender uses F32LE.
                    let audio_floats: &[f32] = bytemuck::cast_slice(audio_data);

                    // We only take channel 0.
                    // We can use `step_by` for a much cleaner iteration of the interleaved samples.
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
                    let bin_width = metadata.sample_rate / (n_samples as f32); // ‼️ Use struct
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
                eprintln!("[AudioReaderFFT] ❌ Error reading: {}", e);
                break;
            }
        }
        // Poll for new data
        thread::sleep(Duration::from_millis(1));
    }
}
