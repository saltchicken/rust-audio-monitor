use proclink::ShmemReader;
use std::{mem, thread, time::Duration};

fn main() {
    let reader = ShmemReader::new("my_synchronized_shmem")
        .expect("Failed to open shared memory. Is the audio_monitor running?");
    println!("[FFT_Reader] Attached to shared memory. Waiting for data...");

    loop {
        match reader.read() {
            Ok(Some(data)) => {
                // ‼️ Must have at least 8 bytes for our metadata
                if data.len() < 8 {
                    println!("[FFT_Reader] ⚠️ Received data is too small for metadata!");
                    continue;
                }

                // Parse metadata
                let sample_rate =
                    f32::from_le_bytes(data[0..4].try_into().expect("Bad sample_rate"));
                let fft_size = u32::from_le_bytes(data[4..8].try_into().expect("Bad fft_size"));

                // Parse magnitudes
                let mag_data = &data[8..];
                let num_bins_received = mag_data.len() / mem::size_of::<f32>();

                let mut peak_mag: f32 = 0.0;
                let mut peak_bin: usize = 0;

                // Iterate over the raw bytes, chunking them into f32s
                for (i, chunk) in mag_data.chunks_exact(4).enumerate() {
                    let mag = f32::from_le_bytes(chunk.try_into().unwrap());
                    if mag > peak_mag {
                        peak_mag = mag;
                        peak_bin = i;
                    }
                }

                // Calculate frequency
                let bin_width = sample_rate / fft_size as f32;
                let peak_freq = peak_bin as f32 * bin_width;

                println!("[FFT_Reader] ✅ Read {} bytes.", data.len());
                println!("  Sample Rate: {} Hz, FFT Size: {}", sample_rate, fft_size);
                println!(
                    "  Bins: {}, Bin Width: {:.2} Hz",
                    num_bins_received, bin_width
                );
                println!(
                    "  🔊 Peak Frequency: {:.2} Hz (Magnitude: {:.2})\n",
                    peak_freq, peak_mag
                );
            }
            Ok(None) => {
                // No new data, just wait.
            }
            Err(e) => {
                eprintln!("[FFT_Reader] ❌ Error reading: {}", e);
                break;
            }
        }
        // Poll for new data instead of just running once
        thread::sleep(Duration::from_millis(1));
    }
}
