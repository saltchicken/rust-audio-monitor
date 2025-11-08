use clap::Parser;
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::param::format::{MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::pod::Pod;
use std::convert::TryInto;
use std::mem;

use hound::{SampleFormat, WavSpec, WavWriter};
use std::io;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, PartialEq, Clone, Copy)]
enum State {
    Listening,
    Recording,
}

struct UserData {
    format: Option<spa::param::audio::AudioInfoRaw>,
    cursor_move: bool,
    state: State,
    buffer: Vec<f32>,
}

#[derive(Parser)]
#[clap(name = "audio-capture", about = "Audio stream capture example")]
struct Opt {
    #[clap(short, long, help = "The target object id to connect to")]
    target: Option<String>,
}

// This is called *after* the state change to avoid blocking the audio thread.
fn save_recording_from_buffer(buffer: Vec<f32>, format: &spa::param::audio::AudioInfoRaw) {
    if buffer.is_empty() {
        println!("Buffer is empty, not saving.");
        return;
    }

    let spec = WavSpec {
        channels: format.channels() as u16,
        sample_rate: format.rate(),
        bits_per_sample: 32, // We are using f32
        sample_format: SampleFormat::Float,
    };

    let filename = "recording.wav";
    println!("Saving recording to {}...", filename);

    match WavWriter::create(filename, spec) {
        Ok(mut writer) => {
            for &sample in &buffer {
                if let Err(e) = writer.write_sample(sample) {
                    eprintln!("Error writing sample: {}", e);
                    break;
                }
            }
            if let Err(e) = writer.finalize() {
                eprintln!("Error finalizing WAV file: {}", e);
            } else {
                println!(
                    "Saved {} samples ({} channels).",
                    buffer.len(),
                    format.channels()
                );
            }
        }
        Err(e) => {
            eprintln!("Error creating WAV file: {}", e);
        }
    }
}

pub fn main() -> Result<(), pw::Error> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;

    let data = Arc::new(Mutex::new(UserData {
        format: None,
        cursor_move: false,
        state: State::Listening,
        buffer: Vec::new(),
    }));

    /* Create a simple stream */
    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
    };
    // uncomment if you want to capture from the sink monitor ports
    props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");

    let stream = pw::stream::StreamBox::new(&core, "audio-capture", props)?;

    let _listener = stream
        .add_local_listener_with_user_data(data.clone())
        .param_changed(|_, user_data_arc, id, param| {
            // NULL means to clear the format
            let Some(param) = param else {
                return;
            };
            if id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }
            let (media_type, media_subtype) = match format_utils::parse_format(param) {
                Ok(v) => v,
                Err(_) => return,
            };
            // only accept raw audio
            if media_type != MediaType::Audio || media_subtype != MediaSubtype::Raw {
                return;
            }

            let mut user_data = user_data_arc.lock().unwrap();
            let mut info = spa::param::audio::AudioInfoRaw::new();
            info.parse(param)
                .expect("Failed to parse param changed to AudioInfoRaw");

            println!(
                "capturing rate:{} channels:{}",
                info.rate(),
                info.channels()
            );
            user_data.format = Some(info);
        })
        .process(|stream, user_data_arc| {
            let mut user_data = user_data_arc.lock().unwrap();

            let Some(format) = user_data.format.as_ref() else {
                return;
            };

            match stream.dequeue_buffer() {
                None => println!("out of buffers"),
                Some(mut buffer) => {
                    let datas = buffer.datas_mut();
                    if datas.is_empty() {
                        return;
                    }
                    let data = &mut datas[0];
                    let n_channels = format.channels();
                    let n_samples = data.chunk().size() / (mem::size_of::<f32>() as u32);
                    if let Some(samples) = data.data() {
                        if user_data.cursor_move {
                            print!("\x1B[{}A", n_channels + 1);
                        }
                        println!("captured {} samples", n_samples / n_channels);

                        // Parse all samples into a temporary Vec
                        let mut all_samples = Vec::with_capacity(n_samples as usize);
                        for n in 0..(n_samples as usize) {
                            let start = n * mem::size_of::<f32>();
                            let end = start + mem::size_of::<f32>();
                            let chan = &samples[start..end];
                            all_samples.push(f32::from_le_bytes(chan.try_into().unwrap()));
                        }

                        // If recording, add samples to the main buffer
                        if user_data.state == State::Recording {
                            user_data.buffer.extend_from_slice(&all_samples);
                        }

                        // --- Metering logic (unchanged, but uses `all_samples`) ---
                        for c in 0..n_channels {
                            let mut max: f32 = 0.0;
                            for n in (c as usize..n_samples as usize).step_by(n_channels as usize) {
                                let f = all_samples[n];
                                max = max.max(f.abs());
                            }
                            let peak = ((max * 30.0) as usize).clamp(0, 39);
                            println!(
                                "channel {}: |{:>w1$}{:w2$}| peak:{}",
                                c,
                                "*",
                                "",
                                max,
                                w1 = peak + 1,
                                w2 = 40 - peak
                            );
                        }
                        user_data.cursor_move = true;
                    }
                }
            }
        })
        .register()?;

    /* Make one parameter with the supported formats. */
    let mut audio_info = spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
    let obj = pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .unwrap()
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).unwrap()];

    /* Now connect this stream. */
    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    // Spawn a new thread to listen for stdin input
    let input_data = data.clone(); // Clone Arc for the new thread
    thread::spawn(move || {
        println!("Capturing audio. Press Enter to start/stop recording...");
        let stdin = io::stdin();
        let mut buffer = String::new(); // Use a string buffer for read_line

        // Loop and block on read_line
        while stdin.read_line(&mut buffer).is_ok() {
            let mut user_data = input_data.lock().unwrap();
            match user_data.state {
                State::Listening => {
                    if user_data.format.is_none() {
                        println!("\n*** Audio format not yet known. Wait a moment. ***");
                        continue; // Don't toggle, just wait for next Enter
                    }
                    user_data.state = State::Recording;
                    user_data.buffer.clear();
                    println!("\n*** STATE: RECORDING *** (Press Enter to stop)");
                }
                State::Recording => {
                    user_data.state = State::Listening;
                    println!("\n*** STATE: LISTENING *** (Saving...)");

                    // Swap buffers to release lock quickly
                    let buffer_to_save = std::mem::take(&mut user_data.buffer);
                    // Clone format info so we can release the lock
                    let format_to_save = *user_data.format.as_ref().unwrap();

                    // Drop the lock *before* file I/O
                    drop(user_data);

                    save_recording_from_buffer(buffer_to_save, &format_to_save);
                }
            }
            buffer.clear(); // Clear buffer for the next read_line
        }
    });

    mainloop.run();

    Ok(())
}
