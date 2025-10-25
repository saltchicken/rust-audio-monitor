use proclink::ShmemReader;
use std::{mem, thread, time::Duration};

fn main() {
    let reader = ShmemReader::new("my_synchronized_shmem")
        .expect("Failed to open shared memory. Is the audio_monitor running?");

    println!("[AudioReader] Attached to shared memory. Waiting for data...");

    loop {
        match reader.read() {
            Ok(Some(data)) => {
                // Must have at least 12 bytes for our metadata
                // (f32 sample_rate + u32 n_channels + u32 n_samples_per_channel)
                if data.len() < 12 {
                    println!("[AudioReader] ⚠️ Received data is too small for metadata!");
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

                // Print the info
                println!("[AudioReader] ✅ Read {} bytes total.", data.len());
                println!("  Sample Rate: {} Hz", sample_rate);
                println!("  Channels: {}", n_channels);
                println!("  Samples per Channel: {}", n_samples_per_channel);
                println!(
                    "  Audio Data Bytes: {} (Expected: {})",
                    audio_data_len, expected_bytes
                );
                println!("  Total Floats Received: {}\n", num_floats_received);

                // Simple validation
                if audio_data_len != expected_bytes {
                    println!(
                        "[AudioReader] ⚠️ WARNING: Received audio data size does not match metadata!"
                    );
                }
            }
            Ok(None) => {
                // No new data, just wait.
            }
            Err(e) => {
                eprintln!("[AudioReader] ❌ Error reading: {}", e);
                break;
            }
        }
        // Poll for new data
        thread::sleep(Duration::from_millis(1));
    }
}
