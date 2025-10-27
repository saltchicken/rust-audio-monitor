use clap::Parser;
use pipelink_audio_lib::{AudioReadError, AudioReader, METADATA_SIZE};
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
    // ‼️ Use the new AudioReader
    let reader = AudioReader::new(&args.name)
        .expect("Failed to open shared memory. Is the audio_monitor running?");
    println!("[AudioReader] Attached to shared memory. Waiting for data...");

    loop {
        // ‼️ Call the new read method
        match reader.read() {
            Ok(Some(data)) => {
                let audio_data_len = mem::size_of_val(data.audio);

                let total_bytes = METADATA_SIZE + audio_data_len;

                println!("[AudioReader] ✅ Read {} bytes total.", total_bytes);
                println!("  Sample Rate: {} Hz", data.metadata.sample_rate);
                println!("  Channels: {}", data.metadata.n_channels);
                println!(
                    "  Samples per Channel: {}",
                    data.metadata.n_samples_per_channel
                );
                println!(
                    "  Audio Data Bytes: {} (Expected: {})",
                    audio_data_len,
                    audio_data_len // They will match due to check in read()
                );
                println!("  Total Floats Received: {}\n", data.audio.len());
            }
            Ok(None) => {
                // No new data, just wait.
            }
            Err(e) => {
                // ‼️ Handle the new, more descriptive error types
                match e {
                    AudioReadError::Shmem(shmem_err) => {
                        eprintln!("[AudioReader] ❌ Shared memory error: {}", shmem_err);
                        break; // Exit on critical SHM error
                    }
                    AudioReadError::DataTooSmall { needed, got } => {
                        eprintln!(
                            "[AudioReader] ⚠️ Parse error: Data too small. Needed {}, got {}",
                            needed, got
                        );
                    }
                    AudioReadError::DataMismatch { expected, got } => {
                        eprintln!(
                            "[AudioReader] ⚠️ Parse error: Data mismatch. Expected {} audio bytes, got {}",
                            expected, got
                        );
                    }
                    AudioReadError::InvalidMetadata(_) | AudioReadError::InvalidAudioData(_) => {
                        eprintln!(
                            "[AudioReader] ⚠️ Parse error: Data is corrupted and cannot be cast."
                        );
                    }
                }
            }
        }
        // Poll for new data
        thread::sleep(Duration::from_millis(1));
    }
}
