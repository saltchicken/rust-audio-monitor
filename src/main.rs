use clap::Parser;
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::param::format::{MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::pod::Pod;
// ‼️ Removed unused TryInto
use proclink::ShmemWriter;
use std::mem;

struct UserData {
    format: spa::param::audio::AudioInfoRaw,
    cursor_move: bool,
    writer: ShmemWriter,
    payload_buffer: Vec<u8>,
}

#[derive(Parser)]
#[clap(name = "audio-capture", about = "Audio stream capture example")]
struct Opt {
    #[clap(short, long, help = "The target object id to connect to")]
    target: Option<String>,
}

pub fn main() -> Result<(), pw::Error> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;

    // Initialize the writer
    const PAYLOAD_SIZE: usize = 16384;
    let writer = ShmemWriter::new("my_synchronized_shmem", PAYLOAD_SIZE)
        .expect("Failed to open or create shared memory");
    println!("[AudioMonitor] Attached to shared memory.");

    let data = UserData {
        format: Default::default(),
        cursor_move: false,
        writer,
        payload_buffer: Vec::new(),
    };

    let props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
    };

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
                let n_samples_total = valid_audio_size_bytes / (mem::size_of::<f32>() as usize);
                let n_samples_per_channel = n_samples_total / n_channels;

                if n_samples_per_channel == 0 {
                    return;
                }

                if let Some(samples) = data.data() {
                    // 1. Define metadata
                    // We will send: sample_rate (f32), n_channels (u32), n_samples_per_channel (u32)
                    let sample_rate = user_data.format.rate() as f32;
                    let n_channels_u32 = user_data.format.channels(); // This is already u32
                    let n_samples_per_channel_u32 = n_samples_per_channel as u32;

                    // 2. Calculate sizes
                    let metadata_size = mem::size_of::<f32>() // sample_rate
                        + mem::size_of::<u32>() // n_channels
                        + mem::size_of::<u32>(); // n_samples_per_channel

                    // ‼️ --- THIS IS THE FIX ---
                    // ‼️ Use the correct `valid_audio_size_bytes` from data.chunk()
                    let audio_data_size = valid_audio_size_bytes;
                    // ‼️ --- END OF FIX ---

                    let payload_size = metadata_size + audio_data_size;

                    // 3. Check if payload fits
                    if payload_size > (PAYLOAD_SIZE - proclink::DATA_INDEX) {
                        // Can't print in RT thread, but this is a critical error
                        // We'll just skip this buffer
                        println!("[AudioMonitor] ⚠️ Payload too large, skipping buffer.");
                        return;
                    }

                    // ‼️ 4. Build the payload in the reusable buffer
                    user_data.payload_buffer.resize(payload_size, 0);

                    // ‼️ Write metadata
                    let mut offset = 0;
                    user_data.payload_buffer[offset..offset + 4]
                        .copy_from_slice(&sample_rate.to_le_bytes());
                    offset += 4;

                    user_data.payload_buffer[offset..offset + 4]
                        .copy_from_slice(&n_channels_u32.to_le_bytes());
                    offset += 4;

                    user_data.payload_buffer[offset..offset + 4]
                        .copy_from_slice(&n_samples_per_channel_u32.to_le_bytes());
                    offset += 4;

                    // ‼️ Write raw audio data
                    // ‼️ This line is now correct, because `audio_data_size` is correct.
                    // ‼️ It will slice `samples` to the valid chunk (e.g., [0..8192])
                    let valid_sample_slice = &samples[0..audio_data_size];
                    user_data.payload_buffer[offset..offset + audio_data_size]
                        .copy_from_slice(valid_sample_slice);

                    // ‼️ 5. Write the complete payload to shared memory
                    match user_data.writer.write(&user_data.payload_buffer) {
                        Ok(true) => {
                            if user_data.cursor_move {
                                print!("\x1B[1A"); // Move up 1 line
                            }
                            println!(
                                "[AudioMonitor] ✅ Wrote {} bytes ({} samples @ {} Hz, {} ch).",
                                payload_size, n_samples_per_channel, sample_rate, n_channels_u32
                            );
                            user_data.cursor_move = true;
                        }
                        Ok(false) => {
                            // Can't print in RT thread, but we'll try for debugging
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

    // ... (This section is unchanged) ...
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
