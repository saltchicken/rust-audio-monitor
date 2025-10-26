use clap::Parser;
use pipelink_audio_lib::{AudioMetadata, METADATA_SIZE};
use proclink::ShmemReader;
use std::{mem, thread, time::Duration};

#[derive(Parser)]
#[clap(name = "audio-reader")]
struct Args {
    #[clap(
        long,
        help = "Name of the shared memory to read from",
        default_value = "pipelink_audio_shmem"
    )]
    name: String,
}

fn main() {
    let args = Args::parse();
    let reader = ShmemReader::new(&args.name)
        .expect("Failed to open shared memory. Is the audio_monitor running?");
    println!("[AudioReader] Attached to shared memory. Waiting for data...");

    loop {
        match reader.read() {
            Ok(Some(data)) => {
                // 1. Check if we have enough data for the metadata header
                if data.len() < METADATA_SIZE {
                    println!(
                        "[AudioReader] ⚠️ Received data is too small for metadata! Need {}, got {}",
                        METADATA_SIZE,
                        data.len()
                    );
                    continue;
                }

                // 2. Split the data into metadata and audio
                let (metadata_bytes, audio_data) = data.split_at(METADATA_SIZE);

                // 3. Cast the metadata bytes into our struct
                // This is a zero-copy operation!
                let metadata: &AudioMetadata = bytemuck::from_bytes(metadata_bytes);

                // 4. Get audio data info
                let audio_data_len = audio_data.len();

                // 5. Calculate expected vs. received
                let expected_bytes = (metadata.n_samples_per_channel
                    * metadata.n_channels
                    * mem::size_of::<f32>() as u32) as usize;

                let num_floats_received = audio_data_len / mem::size_of::<f32>();

                // Print the info
                println!("[AudioReader] ✅ Read {} bytes total.", data.len());
                println!("  Sample Rate: {} Hz", metadata.sample_rate);
                println!("  Channels: {}", metadata.n_channels);
                println!("  Samples per Channel: {}", metadata.n_samples_per_channel);
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
