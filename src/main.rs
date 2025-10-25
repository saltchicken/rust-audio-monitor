use clap::Parser;
use num_complex::Complex;
use pipewire as pw;
use pw::{properties::properties, spa};
use rustfft::{Fft, FftPlanner};
use spa::param::format::{MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::pod::Pod;
use std::convert::TryInto;
use std::mem;
use std::sync::Arc;

// ‼️ Import the shmem writer from your proclink library
use proclink::ShmemWriter;

struct UserData {
    format: spa::param::audio::AudioInfoRaw,
    cursor_move: bool,
    // --- FFT Data ---
    fft_planner: FftPlanner<f32>,
    fft: Option<Arc<dyn Fft<f32>>>,
    fft_size: usize,
    fft_input: Vec<Complex<f32>>,
    fft_output: Vec<Complex<f32>>,
    // --- End FFT Data ---

    // ‼️ Add the writer and a reusable payload buffer
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

    // ‼️ Initialize the writer
    const PAYLOAD_SIZE: usize = 4096;
    let writer = ShmemWriter::new("my_synchronized_shmem", PAYLOAD_SIZE)
        .expect("Failed to open or create shared memory");
    println!("[AudioMonitor] Attached to shared memory.");

    let data = UserData {
        format: Default::default(),
        cursor_move: false,
        fft_planner: FftPlanner::new(),
        fft: None,
        fft_size: 0,
        fft_input: Vec::new(),
        fft_output: Vec::new(),
        // ‼️ Add writer and buffer to UserData
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
                let n_samples_total =
                    (data.chunk().size() / (mem::size_of::<f32>() as u32)) as usize;
                let n_samples_per_channel = n_samples_total / n_channels;
                if n_samples_per_channel == 0 {
                    return;
                }
                // --- Start FFT ---
                // 1. (Re)initialize FFT plan
                if user_data.fft_size != n_samples_per_channel {
                    println!("Initializing FFT for size {}", n_samples_per_channel);
                    user_data.fft = Some(
                        user_data
                            .fft_planner
                            .plan_fft_forward(n_samples_per_channel),
                    );
                    user_data.fft_size = n_samples_per_channel;
                    user_data
                        .fft_input
                        .resize(n_samples_per_channel, Complex::default());
                    user_data
                        .fft_output
                        .resize(n_samples_per_channel, Complex::default());
                }
                let Some(fft) = user_data.fft.as_ref() else {
                    return;
                };
                if let Some(samples) = data.data() {
                    // 2. De-interleave and fill the input buffer (Channel 0)
                    let channel_to_process = 0;
                    let mut sample_count = 0;
                    for n in (channel_to_process..n_samples_total).step_by(n_channels) {
                        if sample_count >= n_samples_per_channel {
                            break;
                        }
                        let start = n * mem::size_of::<f32>();
                        let end = start + mem::size_of::<f32>();
                        if end > samples.len() {
                            break;
                        }
                        let chan_bytes = &samples[start..end];
                        let f = f32::from_le_bytes(chan_bytes.try_into().unwrap());
                        user_data.fft_input[sample_count].re = f;
                        user_data.fft_input[sample_count].im = 0.0;
                        sample_count += 1;
                    }
                    // 3. Run the FFT
                    user_data.fft_output.copy_from_slice(&user_data.fft_input);
                    fft.process(&mut user_data.fft_output);

                    // ‼️ 4. Prepare data for shared memory
                    let num_bins_to_send = n_samples_per_channel / 2;
                    let sample_rate = user_data.format.rate() as f32;
                    let fft_size = user_data.fft_size as u32;

                    // ‼️ Metadata: 1. sample_rate (f32), 2. fft_size (u32)
                    let metadata_size = mem::size_of::<f32>() + mem::size_of::<u32>();
                    let mags_size = num_bins_to_send * mem::size_of::<f32>();
                    let payload_size = metadata_size + mags_size;

                    // ‼️ Check if payload fits (using constants from proclink)
                    if payload_size > (PAYLOAD_SIZE - proclink::DATA_INDEX) {
                        // Can't print in RT thread, but this is a critical error
                        // We'll just skip this buffer
                        return;
                    }

                    // ‼️ Ensure our reusable buffer is the correct size
                    user_data.payload_buffer.resize(payload_size, 0);

                    // ‼️ Write metadata
                    user_data.payload_buffer[0..4].copy_from_slice(&sample_rate.to_le_bytes());
                    user_data.payload_buffer[4..8].copy_from_slice(&fft_size.to_le_bytes());

                    // ‼️ Write magnitudes
                    let mut offset = 8;
                    for complex_val in user_data.fft_output[0..num_bins_to_send].iter() {
                        let magnitude = complex_val.norm();
                        let mag_bytes = magnitude.to_le_bytes();
                        user_data.payload_buffer[offset..offset + 4].copy_from_slice(&mag_bytes);
                        offset += 4;
                    }

                    // ‼️ 5. Write the payload to shared memory
                    match user_data.writer.write(&user_data.payload_buffer) {
                        Ok(true) => {
                            if user_data.cursor_move {
                                print!("\x1B[1A"); // Move up 1 line
                            }
                            println!(
                                "[AudioMonitor] ✅ Wrote {} bytes ({} bins @ {} Hz).",
                                payload_size, num_bins_to_send, sample_rate
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
