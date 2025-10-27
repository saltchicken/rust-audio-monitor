use clap::Parser;
use pipelink_audio_lib::AudioWriterArgs; // ‼️ Use the lib struct

pub fn main() -> Result<(), pipewire::Error> {
    // ‼️ Parse the args defined in the library
    let args = AudioWriterArgs::parse();
    // ‼️ Call the public writer function from the library
    pipelink_audio_lib::run_writer(args)
}
