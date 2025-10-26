use clap::Parser;
use pipelink_audio_lib::{AudioMetadata, METADATA_SIZE};
use pipewire as pw;
use proclink::ShmemWriter;
use pw::{properties::properties, spa};
use spa::param::format::{MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::pod::Pod;
use std::mem;

struct UserData {
    format: spa::param::audio::AudioInfoRaw,
    cursor_move: bool,
    writer: ShmemWriter,
    payload_buffer: Vec<u8>,
}

#[derive(Parser)]
#[clap(name = "pipelink-audio", about = "Audio stream capture example")]
struct Opt {
    #[clap(short, long, help = "The target object id to connect to")]
    target: Option<String>,
    #[clap(
        long,
        help = "Capture from an input source (e.g., mic) instead of an output sink (default)"
    )]
    input: bool,
    #[clap(
        long,
        help = "Name for the shared memory file",
        default_value = "pipelink_audio_shmem"
    )]
    name: String,
}

pub fn main() -> Result<(), pw::Error> {
    let opt = Opt::parse();
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;
    // Initialize the writer
    const PAYLOAD_SIZE: usize = 16384;
    let writer =
        ShmemWriter::new(&opt.name, PAYLOAD_SIZE).expect("Failed to open or create shared memory");
    println!("[AudioMonitor] Attached to shared memory.");
    let data = UserData {
        format: Default::default(),
        cursor_move: false,
        writer,
        payload_buffer: Vec::new(),
    };
    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
    };
    if !opt.input {
        props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");
        println!(
            "[AudioMonitor] Capturing from SINK (output). Use --input to capture from a SOURCE (e.g., mic)."
        );
    } else {
        println!("[AudioMonitor] Capturing from SOURCE (input).");
    }
    let stream = pw::stream::StreamBox::new(&core, "audio-capture", props)?;
    let _listener = stream
        .add_local_listener_with_user_data(data)
        .param_changed(|_, user_data, id, param| {
            // ... (This callback is unchanged) ...
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
            user_data
                .format
                .parse(param)
                .expect("Failed to parse param changed to AudioInfoRaw");
            println!(
                "capturing rate:{} channels:{}",
                user_data.format.rate(),
                user_data.format.channels()
            );
        })
        .process(|stream, user_data| match stream.dequeue_buffer() {
            None => println!("out of buffers"),
            Some(mut buffer) => {
                let datas = buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }
                let data = &mut datas[0];
                let n_channels = user_data.format.channels() as usize;
                if n_channels == 0 {
                    return;
                }
                let valid_audio_size_bytes = data.chunk().size() as usize;
                let n_samples_total = valid_audio_size_bytes / (mem::size_of::<f32>());
                let n_samples_per_channel = n_samples_total / n_channels;
                if n_samples_per_channel == 0 {
                    return;
                }
                if let Some(samples) = data.data() {
                    // 1. Define metadata using our shared struct
                    let metadata = AudioMetadata {
                        sample_rate: user_data.format.rate() as f32,
                        n_channels: user_data.format.channels(), // This is already u32
                        n_samples_per_channel: n_samples_per_channel as u32,
                    };

                    // 2. Calculate sizes
                    let metadata_size = METADATA_SIZE;
                    let audio_data_size = valid_audio_size_bytes;
                    let payload_size = metadata_size + audio_data_size;

                    // 3. Check if payload fits
                    if payload_size > (PAYLOAD_SIZE - proclink::DATA_INDEX) {
                        println!("[AudioMonitor] ⚠️ Payload too large, skipping buffer.");
                        return;
                    }

                    // 4. Build the payload in the reusable buffer
                    user_data.payload_buffer.resize(payload_size, 0);

                    // Write metadata using bytemuck::bytes_of
                    user_data.payload_buffer[0..metadata_size]
                        .copy_from_slice(bytemuck::bytes_of(&metadata));

                    // Write raw audio data
                    let valid_sample_slice = &samples[0..audio_data_size];
                    user_data.payload_buffer[metadata_size..payload_size]
                        .copy_from_slice(valid_sample_slice);

                    // 5. Write the complete payload to shared memory
                    match user_data.writer.write(&user_data.payload_buffer) {
                        Ok(true) => {
                            if user_data.cursor_move {
                                print!("\x1B[1A"); // Move up 1 line
                            }
                            println!(
                                "[AudioMonitor] ✅ Wrote {} bytes ({} samples @ {} Hz, {} ch).",
                                payload_size,
                                n_samples_per_channel,
                                metadata.sample_rate,
                                metadata.n_channels
                            );
                            user_data.cursor_move = true;
                        }
                        Ok(false) => {
                            println!("[AudioMonitor] ⚠️ Failed to write to shared memory.");
                        }
                        Err(_) => {
                            // Error. Can't print in RT thread.
                        }
                    }
                }
            }
        })
        .register()?;

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
    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;
    mainloop.run();
    Ok(())
}
