use pipelink_audio_lib::{AudioReadError, AudioReader, AudioWriterArgs, run_writer};
use std::{thread, time::Duration};

// ‼️ --- START: Added imports for WAV writing ---
use hound::{SampleFormat, WavSpec, WavWriter};
use std::fs::File;
use std::io::BufWriter;
// ‼️ --- END: Added imports for WAV writing ---

fn main() {
    println!("--- starting simple pipelink demo ---");

    // ‼️ use a unique name for this demo's shared memory
    let shmem_name = "pipelink_simple_demo";

    // 1. --- setup writer ---
    // we use default arguments for simplicity, overriding only the shmem name.
    let writer_args = AudioWriterArgs {
        target: None,
        input: false, // capture output (default)
        name: shmem_name.to_string(),
    };

    // 2. --- run writer in a new thread ---
    println!("[demo] launching writer thread...");
    thread::spawn(move || {
        println!("[writer] thread started. running writer...");
        if let Err(e) = run_writer(writer_args) {
            eprintln!("[writer] ❌ error: {}", e);
        }
        println!("[writer] thread finished.");
    });

    // 3. --- wait for writer ---
    // give the writer thread a moment to start, connect to pipewire,
    // and create the shared memory file.
    println!("[demo] waiting for writer to initialize (2s)...");
    thread::sleep(Duration::from_secs(2));

    // 4. --- run reader in main thread ---
    println!("[demo] initializing reader on main thread...");
    let reader = match AudioReader::new(shmem_name) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[reader] ❌ failed to open shared memory: {}", e);
            eprintln!("[reader] ❌ is pipewire running? did the writer thread start correctly?");
            return;
        }
    };

    println!("[reader] ✅ attached to shmem. listening for audio data...");

    let mut frame_count = 0u64;

    // ‼️ --- START: WAV Writer Setup ---
    // We will initialize this on the first frame, since we need the
    // channel count and sample rate from the metadata.
    let mut wav_writer: Option<WavWriter<BufWriter<File>>> = None;
    // ‼️ --- END: WAV Writer Setup ---

    loop {
        match reader.read() {
            Ok(Some(data)) => {
                frame_count += 1;

                // ‼️ --- START: WAV WRITING LOGIC ---
                // If this is the first frame, initialize the writer
                if wav_writer.is_none() {
                    println!("\n[writer] ‼️ First frame! Initializing output.wav...");
                    let spec = WavSpec {
                        // Use metadata from the first frame
                        channels: data.metadata.n_channels as u16,
                        sample_rate: data.metadata.sample_rate as u32,
                        // pipelink gives f32 samples, so we use 32 bits
                        bits_per_sample: 32,
                        sample_format: SampleFormat::Float,
                    };

                    let writer = match WavWriter::create("output.wav", spec) {
                        Ok(w) => w,
                        Err(e) => {
                            eprintln!("\n[writer] ❌ Failed to create WAV file: {}", e);
                            break; // Exit loop
                        }
                    };
                    wav_writer = Some(writer);
                }

                // Write the audio samples to the file
                if let Some(writer) = wav_writer.as_mut() {
                    // Iterate over the interleaved f32 samples and write them
                    println!(
                        "\n[writer] Writing {} samples to output.wav...",
                        data.audio.len()
                    );
                    for sample in data.audio.iter() {
                        if let Err(e) = writer.write_sample(*sample) {
                            eprintln!("\n[writer] ❌ Failed to write sample: {}", e);
                            break; // Stop writing on error
                        }
                    }
                }
                // ‼️ --- END: WAV WRITING LOGIC ---

                let total_samples = data.audio.len();

                // print a one-line summary
                print!(
                    "\r[reader] ✅ frame {}: received {} samples ({} per/ch * {} ch) @ {} hz    ",
                    frame_count,
                    total_samples,
                    data.metadata.n_samples_per_channel,
                    data.metadata.n_channels,
                    data.metadata.sample_rate
                );
                // flush stdout to make the \r work correctly
                use std::io::Write; // ‼️ Corrected import (was lowercase 'write')
                std::io::stdout().flush().unwrap();
            }
            Ok(None) => {
                // no new data, just wait.
            }
            Err(e) => {
                // handle errors
                match e {
                    AudioReadError::Shmem(shmem_err) => {
                        eprintln!("\n[reader] ❌ shared memory error: {}", shmem_err);
                        break; // exit on critical shm error
                    }
                    _ => {
                        eprintln!("\n[reader] ⚠️ read error: {}", e);
                        // ‼️ Don't break here, just log the error
                        // e.g., an overrun is not fatal
                    }
                }
            }
        }
        // poll for new data
        thread::sleep(Duration::from_millis(1));
    }

    // ‼️ --- START: Finalize WAV File ---
    // This runs after the loop breaks (e.g., on Ctrl+C or fatal error)
    println!("\n[demo] Loop finished. Finalizing WAV file...");
    if let Some(writer) = wav_writer {
        if let Err(e) = writer.finalize() {
            eprintln!("[writer] ❌ Failed to finalize WAV file: {}", e);
        } else {
            println!("[writer] ✅ Successfully saved to output.wav");
        }
    }
    // ‼️ --- END: Finalize WAV File ---
}
