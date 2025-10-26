use proclink::ShmemReader;
use std::{mem, thread, time::Duration};

// ‼️ --- Added for FFT ---
use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::sync::Arc;
// ‼️ --- End FFT Imports ---

fn main() {
    let reader = ShmemReader::new("my_synchronized_shmem")
        .expect("Failed to open shared memory. Is the audio_monitor running?");

    println!("[AudioReaderFFT] Attached to shared memory. Waiting for data...");

    // ‼️ --- Added for FFT ---
    // We create a planner and a reusable plan variable.
    // This is much more efficient than creating a new FFT plan for every audio buffer.
    let mut planner = FftPlanner::new();
    let mut fft_plan: Option<(usize, Arc<dyn Fft<f32>>)> = None;
    let mut complex_buffer: Vec<Complex<f32>> = Vec::new();
    // ‼️ --- End FFT Setup ---

    loop {
        match reader.read() {
            Ok(Some(data)) => {
                // Must have at least 12 bytes for our metadata
                // (f32 sample_rate + u32 n_channels + u32 n_samples_per_channel)
                if data.len() < 12 {
                    println!("[AudioReaderFFT] ⚠️ Received data is too small for metadata!");
                    continue;
                }

                // Parse new metadata
                let sample_rate =
                    f32::from_le_bytes(data[0..4].try_into().expect("Bad sample_rate"));
                let n_channels = u32::from_le_bytes(data[4..8].try_into().expect("Bad n_channels"));
                let n_samples_per_channel =
                    u32::from_le_bytes(data[8..12].try_into().expect("Bad n_samples_per_channel"));

                // Get audio data
                let audio_data = &data[12..];
                let audio_data_len = audio_data.len();

                // Calculate expected vs. received
                let expected_bytes =
                    (n_samples_per_channel * n_channels * mem::size_of::<f32>() as u32) as usize;
                let num_floats_received = audio_data_len / mem::size_of::<f32>();

                // Print the info (as before)
                println!("[AudioReaderFFT] ✅ Read {} bytes total.", data.len());
                println!("  Sample Rate: {} Hz", sample_rate);
                println!("  Channels: {}", n_channels);
                println!("  Samples per Channel: {}", n_samples_per_channel);
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

                // ‼️ --- Start FFT Calculation ---
                if n_samples_per_channel > 0 && n_channels > 0 {
                    let n_samples = n_samples_per_channel as usize;
                    let n_chans_usize = n_channels as usize;

                    // 1. Get or create the FFT plan for the current buffer size
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

                    // 2. Prepare the complex buffer. We only take channel 0 (the first channel).
                    complex_buffer.clear();
                    complex_buffer.resize(n_samples, Complex::default());

                    for i in 0..n_samples {
                        let sample_start_byte = i * n_chans_usize * mem::size_of::<f32>();
                        let sample_end_byte = sample_start_byte + mem::size_of::<f32>();

                        if sample_end_byte > audio_data.len() {
                            break; // Avoid panic on incomplete data
                        }

                        // Read the f32 sample for channel 0
                        let sample_bytes: [u8; 4] = audio_data[sample_start_byte..sample_end_byte]
                            .try_into()
                            .unwrap();
                        let sample_f32 = f32::from_le_bytes(sample_bytes);

                        complex_buffer[i] = Complex {
                            re: sample_f32,
                            im: 0.0,
                        };
                    }

                    // 3. Run the FFT
                    fft.process(&mut complex_buffer);

                    // 4. Find the peak frequency
                    // We only need to check the first half of the bins (due to Nyquist theorem)
                    let (peak_bin_index, peak_magnitude) = complex_buffer[..n_samples / 2] // Only check first half
                        .iter()
                        .enumerate()
                        .map(|(i, c)| (i, c.norm())) // Get index and magnitude (c.norm() is sqrt(re^2 + im^2))
                        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                        .unwrap_or((0, 0.0));

                    // 5. Convert the bin index back to a frequency
                    let bin_width = sample_rate / (n_samples as f32);
                    let peak_frequency = peak_bin_index as f32 * bin_width;

                    println!(
                        "  [FFT] ‼️ Peak Frequency (Ch 0): {:.2} Hz (Magnitude: {:.2})\n",
                        peak_frequency, peak_magnitude
                    );
                }
                // ‼️ --- End FFT Calculation ---
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
