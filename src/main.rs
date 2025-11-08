use hound::{SampleFormat, WavSpec, WavWriter};
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::param::format::{MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::pod::Pod;
use std::convert::TryInto;
use std::mem;
// ‼️ Added/changed imports for IPC
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, PartialEq, Clone)] // ‼️ Removed Copy
enum State {
    Listening,
    // ‼️ Store the save path within the Recording state
    Recording(PathBuf),
}

struct UserData {
    format: Option<spa::param::audio::AudioInfoRaw>,
    state: State,
    buffer: Vec<f32>,
}

// ‼️ Function signature changed to accept a PathBuf
fn save_recording_from_buffer(
    buffer: Vec<f32>,
    format: &spa::param::audio::AudioInfoRaw,
    filename: &Path, // ‼️ Use Path
) {
    if buffer.is_empty() {
        println!("Buffer is empty, not saving.");
        return;
    }

    // ‼️ Create parent directories if they don't exist
    if let Some(parent) = filename.parent() {
        if !parent.exists() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!("Failed to create directory {}: {}", parent.display(), e);
                return;
            }
        }
    }

    let spec = WavSpec {
        channels: format.channels() as u16,
        sample_rate: format.rate(),
        bits_per_sample: 32, // We are using f32
        sample_format: SampleFormat::Float,
    };

    println!("Saving recording to {}...", filename.display()); // ‼️ Use display()
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
                    "Saved {} samples ({} channels) to {}.",
                    buffer.len(),
                    format.channels(),
                    filename.display() // ‼️
                );
            }
        }
        Err(e) => {
            eprintln!("Error creating WAV file: {}", e);
        }
    }
}

// ‼️ This is the new command handler thread
fn start_ipc_listener(data: Arc<Mutex<UserData>>) -> std::io::Result<()> {
    let socket_path = "/tmp/rust-audio-monitor.sock";

    // ‼️ Clean up any old socket file that might exist
    let _ = fs::remove_file(socket_path);

    let listener = UnixListener::bind(socket_path)?;
    println!("Control socket listening at {}", socket_path);

    // ‼️ Accept incoming connections
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();

                // ‼️ Read commands from the client (e.g., "START /foo/bar.wav", "STOP")
                while let Ok(bytes_read) = reader.read_line(&mut line) {
                    if bytes_read == 0 {
                        // ‼️ Client disconnected
                        break;
                    }

                    let parts: Vec<&str> = line.trim().splitn(2, ' ').collect();
                    let command = parts.get(0).unwrap_or(&"");
                    let payload = parts.get(1);

                    let mut user_data = data.lock().unwrap();

                    match *command {
                        "START" => {
                            if let Some(path_str) = payload {
                                if user_data.format.is_none() {
                                    eprintln!("Refused START: Audio format not yet known.");
                                    continue;
                                }
                                match user_data.state {
                                    State::Listening => {
                                        let path = PathBuf::from(path_str);
                                        println!("START recording to {}", path.display());
                                        user_data.state = State::Recording(path);
                                        user_data.buffer.clear();
                                    }
                                    State::Recording(_) => {
                                        eprintln!("Refused START: Already recording.");
                                    }
                                }
                            } else {
                                eprintln!("Invalid START: Missing file path.");
                            }
                        }
                        "STOP" => {
                            // ‼️ Use mem::replace to atomically switch state
                            let old_state =
                                std::mem::replace(&mut user_data.state, State::Listening);

                            if let State::Recording(save_path) = old_state {
                                println!("STOP recording.");
                                let buffer_to_save = std::mem::take(&mut user_data.buffer);
                                let format_to_save = *user_data.format.as_ref().unwrap();

                                // ‼️ Drop lock *before* saving file
                                drop(user_data);

                                save_recording_from_buffer(
                                    buffer_to_save,
                                    &format_to_save,
                                    &save_path,
                                );
                            } else {
                                eprintln!("Refused STOP: Not recording.");
                            }
                        }
                        "STATUS" => {
                            println!("STATUS: {:?}", user_data.state);
                        }
                        _ => {
                            eprintln!("Unknown command: {}", line.trim());
                        }
                    }

                    line.clear(); // ‼️ Clear buffer for next command
                }
            }
            Err(e) => {
                eprintln!("IPC connection failed: {}", e);
            }
        }
    }

    Ok(())
}

pub fn main() -> Result<(), pw::Error> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;

    let data = Arc::new(Mutex::new(UserData {
        format: None,
        state: State::Listening,
        buffer: Vec::new(),
    }));

    /* Create a simple stream */
    let props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::STREAM_CAPTURE_SINK => "true",
    };
    let stream = pw::stream::StreamBox::new(&core, "audio-capture", props)?;

    let _listener = stream
        .add_local_listener_with_user_data(data.clone())
        .param_changed(|_, user_data_arc, id, param| {
            // ... (rest of this closure is unchanged)
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

            // ‼️ Check state *before* processing buffer
            if user_data.state == State::Listening {
                // ‼️ Still dequeue to keep the buffer chain moving, but do nothing with it
                let _ = stream.dequeue_buffer();
                return;
            }

            match stream.dequeue_buffer() {
                None => println!("out of buffers"),
                Some(mut buffer) => {
                    let datas = buffer.datas_mut();
                    if datas.is_empty() {
                        return;
                    }
                    let data = &mut datas[0];
                    let _n_channels = format.channels(); // ‼️ Not needed here
                    let n_samples = data.chunk().size() / (mem::size_of::<f32>() as u32);
                    if let Some(samples) = data.data() {
                        let mut all_samples = Vec::with_capacity(n_samples as usize);
                        for n in 0..(n_samples as usize) {
                            let start = n * mem::size_of::<f32>();
                            let end = start + mem::size_of::<f32>();
                            let chan = &samples[start..end];
                            all_samples.push(f32::from_le_bytes(chan.try_into().unwrap()));
                        }

                        // ‼️ This check is technically redundant now,
                        // but good to keep for clarity.
                        if let State::Recording(_) = user_data.state {
                            user_data.buffer.extend_from_slice(&all_samples);
                        }
                    }
                }
            }
        })
        .register()?;

    /* Make one parameter with the supported formats. */
    // ... (This section is unchanged)
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
    // ... (This section is unchanged)
    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    // ‼️ Spawn the new IPC listener thread
    let ipc_data = data.clone();
    thread::spawn(move || {
        if let Err(e) = start_ipc_listener(ipc_data) {
            eprintln!("IPC listener thread failed: {}", e);
        }
    });

    mainloop.run();

    // ‼️ Clean up the socket file on exit
    let _ = fs::remove_file("/tmp/rust-audio-monitor.sock");
    Ok(())
}
