use clap::Parser;
use pipewire as pw;
use pw::{properties::properties, spa};
use spa::param::format::{MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::pod::Pod;
use std::convert::TryInto;
use std::mem;

// --- FFT Imports ---
use num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;
// --- End FFT Imports ---

struct UserData {
    format: spa::param::audio::AudioInfoRaw,
    cursor_move: bool,
    // --- FFT Data ---
    /// A planner to create new FFT instances
    fft_planner: FftPlanner<f32>,
    /// The FFT algorithm instance
    fft: Option<Arc<dyn Fft<f32>>>,
    /// The size of the FFT (buffer size per channel)
    fft_size: usize,
    /// Re-usable buffer for FFT input
    fft_input: Vec<Complex<f32>>,
    /// Re-usable buffer for FFT output
    fft_output: Vec<Complex<f32>>,
    // --- End FFT Data ---
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

    let data = UserData {
        format: Default::default(),
        cursor_move: false,
        // --- Initialize FFT Data ---
        fft_planner: FftPlanner::new(),
        fft: None,
        fft_size: 0,
        fft_input: Vec::new(),
        fft_output: Vec::new(),
        // --- End FFT Data ---
    };

    /* Create a simple stream, the simple stream manages the core and remote
     * objects for you if you don't need to deal with them.
     *
     * If you plan to autoconnect your stream, you need to provide at least
     * media, category and role properties.
     *
     * Pass your events and a user_data pointer as the last arguments. This
     * will inform you about the stream state. The most important event
     * you need to listen to is the process event where you need to produce
     * the data.
     */
    let props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
    };

    // uncomment if you want to capture from the sink monitor ports
    // props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");

    let stream = pw::stream::StreamBox::new(&core, "audio-capture", props)?;

    let _listener = stream
        .add_local_listener_with_user_data(data)
        .param_changed(|_, user_data, id, param| {
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

            // call a helper function to parse the format for us.
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
                // ‼️ Need to guard against division by zero if format isn't set
                if n_channels == 0 {
                    return;
                }

                let n_samples_total =
                    (data.chunk().size() / (mem::size_of::<f32>() as u32)) as usize;
                // ‼️ This is the number of samples *per channel*, which is our FFT size
                let n_samples_per_channel = n_samples_total / n_channels;
                if n_samples_per_channel == 0 {
                    return;
                }

                // --- Start FFT ---

                // 1. (Re)initialize FFT plan if buffer size changed
                // This avoids re-allocating, but handles dynamic buffer size changes
                if user_data.fft_size != n_samples_per_channel {
                    println!("Initializing FFT for size {}", n_samples_per_channel);
                    user_data.fft = Some(
                        user_data
                            .fft_planner
                            .plan_fft_forward(n_samples_per_channel),
                    );
                    user_data.fft_size = n_samples_per_channel;
                    // ‼️ Resize re-usable buffers
                    user_data
                        .fft_input
                        .resize(n_samples_per_channel, Complex::default());
                    user_data
                        .fft_output
                        .resize(n_samples_per_channel, Complex::default());
                }

                // Get the FFT plan. If it's None (e.g., size is 0), just return.
                let Some(fft) = user_data.fft.as_ref() else {
                    return;
                };

                if let Some(samples) = data.data() {
                    if user_data.cursor_move {
                        print!("\x1B[2A"); // ‼️ We are printing 2 lines now
                    }
                    println!("captured {} samples per channel", n_samples_per_channel);

                    // 2. De-interleave and fill the input buffer
                    // We'll just process the first channel (channel 0) as an example.
                    let channel_to_process = 0;
                    let mut sample_count = 0;

                    for n in (channel_to_process..n_samples_total).step_by(n_channels) {
                        if sample_count >= n_samples_per_channel {
                            break;
                        }

                        let start = n * mem::size_of::<f32>();
                        let end = start + mem::size_of::<f32>();
                        if end > samples.len() {
                            break; // Avoid panic on buffer underrun
                        }

                        let chan_bytes = &samples[start..end];
                        let f = f32::from_le_bytes(chan_bytes.try_into().unwrap());

                        // Load sample into the complex buffer's real part.
                        // NOTE: A window function (e.g., Hann) should be applied here
                        // for real-world applications to reduce spectral leakage.
                        user_data.fft_input[sample_count].re = f;
                        user_data.fft_input[sample_count].im = 0.0; // Ensure imaginary part is 0

                        sample_count += 1;
                    }

                    // 3. Run the FFT
                    // `process` works in-place, so we copy input to output buffer first
                    user_data.fft_output.copy_from_slice(&user_data.fft_input);
                    fft.process(&mut user_data.fft_output);

                    // 4. Process the output (find the peak frequency)
                    // The output is symmetric, so we only need to look at the first half
                    let mut peak_magnitude: f32 = 0.0;
                    let mut peak_bin: usize = 0;

                    for (bin_index, complex_val) in user_data.fft_output
                        [0..n_samples_per_channel / 2] // Only check first half
                        .iter()
                        .enumerate()
                    {
                        let magnitude = complex_val.norm(); // .norm() is sqrt(re^2 + im^2)
                        if magnitude > peak_magnitude {
                            peak_magnitude = magnitude;
                            peak_bin = bin_index;
                        }
                    }

                    // 5. Calculate the actual frequency
                    // freq = bin_index * (sample_rate / fft_size)
                    let sample_rate = user_data.format.rate() as f32;
                    let peak_freq = peak_bin as f32 * (sample_rate / n_samples_per_channel as f32);

                    // Print the peak frequency instead of the volume meter
                    println!(
                        "Peak Frequency: {:.2} Hz (Magnitude: {:.2})",
                        peak_freq, peak_magnitude
                    );
                    user_data.cursor_move = true;

                    // --- End FFT ---
                }
            }
        })
        .register()?;

    /* Make one parameter with the supported formats. The SPA_PARAM_EnumFormat
     * id means that this is a format enumeration (of 1 value).
     * We leave the channels and rate empty to accept the native graph
     * rate and channels. */
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

    /* Now connect this stream. We ask that our process function is
     * called in a realtime thread. */
    stream.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    // and wait while we let things run
    mainloop.run();

    Ok(())
}
